import tailwindcss from '@tailwindcss/vite'
import vue from '@vitejs/plugin-vue'
import { defineConfig, type Plugin, type PreviewServer, type ViteDevServer } from 'vite'
import { existsSync, readFileSync, statSync } from 'node:fs'
import { resolve, sep } from 'node:path'
import {
  LEGACY_REDIRECTS,
  PUBLIC_PAGES,
  isWorkspacePath,
  robotsText,
  sitemapXml,
} from './src/site-pages.ts'

const deployment: { headers?: { source: string; headers: { key: string; value: string }[] }[] } =
  JSON.parse(readFileSync(new URL('./vercel.json', import.meta.url), 'utf8'))
const securityHeaders = Object.fromEntries(
  deployment.headers
    ?.find(({ source }) => source === '/(.*)')
    ?.headers.map(({ key, value }) => [key, value]) ?? [],
)

// Reproduce the hosting route contract in real dev/preview HTTP tests. Production
// routing itself stays native to Vercel/Caddy, with no new hosted middleware.
function indexing(): Plugin {
  function install(server: PreviewServer | ViteDevServer, preview: boolean) {
    const output = resolve(server.config.root, server.config.build.outDir)
    const staticRoot = preview ? output : resolve(server.config.root, 'public')
    server.middlewares.use(async (req, res, next) => {
      const requestUrl = new URL(req.url ?? '/', 'http://vite.local')
      const path = requestUrl.pathname
      if (preview)
        for (const [key, value] of Object.entries(securityHeaders)) res.setHeader(key, value)
      if (
        /^\/(?:api(?:\/|$)|webhooks(?:\/|$)|api-doc(?:\/|$)|docs|health$|ready$|metrics$)/.test(
          path,
        )
      ) {
        res.setHeader('X-Robots-Tag', 'noindex, nofollow')
        return next()
      }
      const destination = LEGACY_REDIRECTS[path]
      if (destination) {
        res.statusCode = 308
        res.setHeader('Location', destination + requestUrl.search)
        return res.end()
      }
      if (path === '/robots.txt' || path === '/sitemap.xml') {
        if (preview) return next() // Test the actual generated files, not a mock.
        res.setHeader(
          'Content-Type',
          path === '/robots.txt' ? 'text/plain; charset=utf-8' : 'application/xml; charset=utf-8',
        )
        return res.end(path === '/robots.txt' ? robotsText() : sitemapXml())
      }
      if (PUBLIC_PAGES.some((page) => page.path === path)) {
        if (preview) req.url = (path === '/' ? '/index.html' : `${path}.html`) + requestUrl.search
        return next()
      }
      if (isWorkspacePath(path)) {
        res.setHeader('X-Robots-Tag', 'noindex, nofollow')
        req.url = (preview ? '/app.html' : '/index.html') + requestUrl.search
        return next()
      }
      const file = resolve(staticRoot, `.${path}`)
      const staticFile =
        file.startsWith(staticRoot + sep) && existsSync(file) && statSync(file).isFile()
      const devModule = !preview && /^\/(?:@|src\/|node_modules\/)/.test(path)
      if (path !== '/404.html' && (staticFile || devModule)) return next()
      res.statusCode = 404
      res.setHeader('X-Robots-Tag', 'noindex, nofollow')
      res.setHeader('Content-Type', 'text/html; charset=utf-8')
      if (preview) return res.end(readFileSync(resolve(output, '404.html')))
      try {
        const dev = server as ViteDevServer
        const html = await dev.transformIndexHtml(
          requestUrl.pathname,
          readFileSync(resolve(server.config.root, 'index.html'), 'utf8'),
        )
        res.end(html)
      } catch (error) {
        next(error)
      }
    })
  }
  return {
    name: 'indexing-routes',
    configureServer: (server) => install(server, false),
    configurePreviewServer: (server) => install(server, true),
  }
}

export default defineConfig({
  build: { manifest: true },
  preview: { headers: securityHeaders },
  plugins: [indexing(), vue(), tailwindcss()],
  server: {
    proxy: {
      '/api': 'http://127.0.0.1:3000',
      '/health': 'http://127.0.0.1:3000',
      '/ready': 'http://127.0.0.1:3000',
    },
  },
})
