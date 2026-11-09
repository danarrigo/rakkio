import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import { router } from './router'
import './style.css'

const app = createApp(App).use(createPinia()).use(router)
// Keep prerendered public content visible until the client route is ready.
void router.isReady().then(() => app.mount('#app'))
