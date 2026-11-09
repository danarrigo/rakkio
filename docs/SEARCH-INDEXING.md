# Public search indexing

Only these official public URLs are intended for search indexing:

- `https://rakkio.com/`
- `https://rakkio.com/privacy`
- `https://rakkio.com/terms`
- `https://rakkio.com/delete-account`

`apps/web/src/site-pages.ts` owns the public metadata, sitemap membership and legacy aliases. `page-metadata.ts` updates browser metadata on client navigation. The web build prerenders only the public components through `entry-public.ts`; it does not render App, account stores, session state or private content. The existing client application takes over after its router is ready. Public HTML stays visible without JavaScript and while session discovery is pending.

The build emits real `robots.txt`/`sitemap.xml`, independent public HTML documents, a non-indexable `app.html` for workspace deep links and a not-found document. Robots allows crawling so crawlers can observe `noindex`; it is not an access-control boundary. The frontend routes private pages to the empty client entry with `X-Robots-Tag: noindex, nofollow`. API responses carry the same header, including 401/404 responses. Authentication and tenant authorization remain independent and unchanged.

Vercel and both Caddy packages rewrite only recognized public/workspace routes. Unknown URLs return actual HTTP404 rather than an unrelated homepage. `/home` and `/home/` permanently redirect to `/`. The old `/delete-account.html` leads to usable public instructions; there is no current blog, so `/blog.html` returns404. Known legal-page HTML/trailing-slash aliases redirect to their canonical paths. HTTP, www and old-domain redirects remain intentional.

The canonical origin is the official site, not a request Host header. Self-hosters publishing different public branding/legal documents should review `SITE_ORIGIN` and the public page contents before building; OAuth/API origin configuration is separate. Private/self-hosted workspace data must not be added to the sitemap.

## Verification

- Public HTTP/no-JavaScript rendering, canonical uniqueness, crawl-file formats, legacy redirects, missing pages and private metadata: `apps/web/e2e/indexing.spec.ts` with `playwright.security.config.ts`.
- Metadata transitions and Vercel alias parity: `src/page-metadata.test.ts`.
- Native Caddy container routes: the remote containers check. Production self-hosting parity: `scripts/personal/smoke.sh`, on a disposable remote runner only.
- API noindex across successful/authentication/error responses: `apps/api/tests/http/security_regressions.rs`, remote backend check.

After deployment, submit `https://rakkio.com/sitemap.xml` in the rakkio.com Search Console property. Inspect the homepage, test the live URL, and request indexing. Recheck Google's selected canonical after recrawling; a declared canonical is a preference, not a guarantee. Do not try to index API roots, private routes, redirecting variants or intentionally removed pages. Existing redirect/404 exclusions can remain in Search Console legitimately, and reports can lag the deployed fixes. No Search Console submission or indexing acceptance is claimed from CI alone.
