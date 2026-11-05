use std::sync::Arc;

use anyhow::{Context, Result};
use bytes::Bytes;
use object_store::{ObjectStore, ObjectStoreExt, path::Path, signer::Signer};

use crate::config::ObjectStorageConfig;

#[derive(Clone)]
pub struct FileStorage {
    store: Arc<dyn ObjectStore>,
    download_signer: Option<Arc<object_store::aws::AmazonS3>>,
}

impl FileStorage {
    pub fn new(config: &ObjectStorageConfig) -> Result<Self> {
        let mut download_signer = None;
        let store: Arc<dyn ObjectStore> = match config {
            ObjectStorageConfig::Local { root } => {
                std::fs::create_dir_all(root)
                    .with_context(|| format!("failed to create file storage directory {root}"))?;
                Arc::new(
                    object_store::local::LocalFileSystem::new_with_prefix(root)
                        .context("failed to configure local file storage")?,
                )
            }
            ObjectStorageConfig::S3 {
                bucket,
                region,
                endpoint,
                public_endpoint,
                access_key_id,
                secret_access_key,
                allow_http,
                virtual_hosted_style,
            } => {
                let backend = Arc::new(build_s3(
                    bucket,
                    region,
                    endpoint.as_deref(),
                    access_key_id,
                    secret_access_key,
                    *allow_http,
                    *virtual_hosted_style,
                )?);
                download_signer = Some(match public_endpoint.as_deref() {
                    Some(public_endpoint) => Arc::new(build_s3(
                        bucket,
                        region,
                        Some(public_endpoint),
                        access_key_id,
                        secret_access_key,
                        *allow_http,
                        *virtual_hosted_style,
                    )?),
                    None => backend.clone(),
                });
                backend
            }
        };
        Ok(Self {
            store,
            download_signer,
        })
    }

    pub async fn put(&self, key: &str, bytes: Bytes) -> Result<()> {
        self.store
            .put(&Path::from(key), bytes.into())
            .await
            .context("failed to store file")?;
        Ok(())
    }

    pub async fn get(&self, key: &str) -> Result<Bytes> {
        self.store
            .get(&Path::from(key))
            .await
            .context("failed to read file")?
            .bytes()
            .await
            .context("failed to download file")
    }

    pub async fn signed_download_url(&self, key: &str) -> Result<Option<String>> {
        let Some(download_signer) = &self.download_signer else {
            return Ok(None);
        };
        download_signer
            .signed_url(
                http::Method::GET,
                &Path::from(key),
                std::time::Duration::from_secs(300),
            )
            .await
            .map(|url| Some(url.to_string()))
            .context("failed to sign file download")
    }

    pub async fn delete(&self, key: &str) -> Result<()> {
        match self.store.delete(&Path::from(key)).await {
            Ok(()) | Err(object_store::Error::NotFound { .. }) => Ok(()),
            Err(error) => Err(error).context("failed to delete file"),
        }
    }
}

fn build_s3(
    bucket: &str,
    region: &str,
    endpoint: Option<&str>,
    access_key_id: &str,
    secret_access_key: &str,
    allow_http: bool,
    virtual_hosted_style: bool,
) -> Result<object_store::aws::AmazonS3> {
    let mut builder = object_store::aws::AmazonS3Builder::new()
        .with_bucket_name(bucket)
        .with_region(region)
        .with_access_key_id(access_key_id)
        .with_secret_access_key(secret_access_key)
        .with_allow_http(allow_http)
        .with_virtual_hosted_style_request(virtual_hosted_style);
    if let Some(endpoint) = endpoint {
        let endpoint = if virtual_hosted_style {
            virtual_hosted_endpoint(bucket, endpoint)?
        } else {
            endpoint.to_owned()
        };
        builder = builder.with_endpoint(endpoint);
    }
    builder
        .build()
        .context("failed to configure S3 file storage")
}

fn virtual_hosted_endpoint(bucket: &str, endpoint: &str) -> Result<String> {
    let mut url = reqwest::Url::parse(endpoint).context("S3 endpoint must be a valid URL")?;
    let host = url
        .host_str()
        .context("S3 endpoint must include a hostname")?
        .to_owned();
    if host != bucket && !host.starts_with(&format!("{bucket}.")) {
        url.set_host(Some(&format!("{bucket}.{host}")))
            .map_err(|_| anyhow::anyhow!("S3 bucket cannot be used as a virtual hostname"))?;
    }
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn signed_download_urls_use_the_browser_facing_endpoint() {
        let storage = FileStorage::new(&ObjectStorageConfig::S3 {
            bucket: "prosepect".to_owned(),
            region: "us-east-1".to_owned(),
            endpoint: Some("http://minio:9000".to_owned()),
            public_endpoint: Some("http://localhost:9000".to_owned()),
            access_key_id: "test-access-key".to_owned(),
            secret_access_key: "test-secret-key".to_owned(),
            allow_http: true,
            virtual_hosted_style: false,
        })
        .expect("storage should be configured");

        let url = storage
            .signed_download_url("test-object")
            .await
            .expect("URL signing should succeed")
            .expect("S3 storage should produce a signed URL");

        assert!(url.starts_with("http://localhost:9000/prosepect/test-object?"));
    }

    #[tokio::test]
    async fn deleting_an_absent_local_object_is_idempotent() {
        let root = std::env::temp_dir().join(format!("rakkio-storage-{}", uuid::Uuid::now_v7()));
        let storage = FileStorage::new(&ObjectStorageConfig::Local {
            root: root.to_string_lossy().into_owned(),
        })
        .unwrap();
        storage.delete("never-created").await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn signed_download_urls_support_virtual_hosted_buckets() {
        let storage = FileStorage::new(&ObjectStorageConfig::S3 {
            bucket: "prosepect-production".to_owned(),
            region: "auto".to_owned(),
            endpoint: Some("https://storage.railway.app".to_owned()),
            public_endpoint: None,
            access_key_id: "test-access-key".to_owned(),
            secret_access_key: "test-secret-key".to_owned(),
            allow_http: false,
            virtual_hosted_style: true,
        })
        .expect("storage should be configured");

        let url = storage
            .signed_download_url("test-object")
            .await
            .expect("URL signing should succeed")
            .expect("S3 storage should produce a signed URL");

        assert!(
            url.starts_with("https://prosepect-production.storage.railway.app/test-object?"),
            "unexpected signed URL: {url}"
        );
    }
}
