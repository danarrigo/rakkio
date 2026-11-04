import { createRouter, createWebHistory } from 'vue-router'

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', name: 'today', component: () => import('./views/TodayView.vue') },
    {
      path: '/inbox',
      name: 'inbox',
      component: () => import('./views/InboxView.vue'),
      meta: { title: 'Inbox' },
    },
    {
      path: '/calendar',
      name: 'calendar',
      component: () => import('./views/CalendarView.vue'),
    },
    {
      path: '/projects',
      name: 'projects',
      component: () => import('./views/ProjectsView.vue'),
    },
    { path: '/notes', name: 'notes', component: () => import('./views/NotesView.vue') },
    {
      path: '/operations',
      name: 'operations',
      component: () => import('./views/OperationsView.vue'),
      meta: { title: 'Operations' },
    },
    { path: '/files', name: 'files', component: () => import('./views/FilesView.vue') },
    {
      path: '/groups',
      name: 'groups',
      component: () => import('./views/GroupsView.vue'),
      meta: { title: 'Groups' },
    },
    {
      path: '/join',
      name: 'group-join',
      component: () => import('./views/GroupJoinView.vue'),
      meta: { public: true, title: 'Group invitation' },
    },
    {
      path: '/settings',
      name: 'settings',
      component: () => import('./views/SettingsView.vue'),
    },
    {
      path: '/privacy',
      name: 'privacy',
      component: () => import('./views/PrivacyView.vue'),
      meta: { public: true, title: 'Privacy Policy' },
    },
    {
      path: '/terms',
      name: 'terms',
      component: () => import('./views/TermsView.vue'),
      meta: { public: true, title: 'Terms of Service' },
    },
    { path: '/:pathMatch(.*)*', redirect: '/' },
  ],
})

router.afterEach((to) => {
  document.title = to.meta.title ? `${String(to.meta.title)} | rakkio` : 'rakkio'
})
