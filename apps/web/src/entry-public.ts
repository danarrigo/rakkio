// Build-time public content only. Never import App, account stores or session state.
import { createSSRApp, h } from 'vue'
import { renderToString } from '@vue/server-renderer'
import { createMemoryHistory, createRouter, RouterView } from 'vue-router'
import PublicHome from './components/PublicHome.vue'
import PrivacyView from './views/PrivacyView.vue'
import TermsView from './views/TermsView.vue'
import DeleteAccountView from './views/DeleteAccountView.vue'
import NotFoundView from './views/NotFoundView.vue'
export { PUBLIC_PAGES, SITE_ORIGIN, robotsText, sitemapXml } from './site-pages'

export async function renderPublicPage(path: string) {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/', component: PublicHome },
      { path: '/privacy', component: PrivacyView },
      { path: '/terms', component: TermsView },
      { path: '/delete-account', component: DeleteAccountView },
      { path: '/:pathMatch(.*)*', component: NotFoundView },
    ],
  })
  await router.push(path)
  await router.isReady()
  return renderToString(createSSRApp({ render: () => h(RouterView) }).use(router))
}
