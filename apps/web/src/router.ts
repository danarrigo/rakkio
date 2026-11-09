import { createRouter, createWebHistory } from 'vue-router'
import { LEGACY_REDIRECTS } from './site-pages'
import { updatePageMetadata } from './page-metadata'

export const router = createRouter({
  history: createWebHistory(),
  routes: [
    ...Object.entries(LEGACY_REDIRECTS)
      .filter(([path]) => !path.endsWith('/'))
      .map(([path, redirect]) => ({ path, redirect })),
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
      path: '/groups/:groupId/assignments',
      name: 'group-assignments',
      component: () => import('./views/GroupAssignmentsView.vue'),
      meta: { title: 'Group assignments' },
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
    {
      path: '/delete-account',
      name: 'delete-account',
      component: () => import('./views/DeleteAccountView.vue'),
      meta: { public: true, title: 'Delete your account' },
    },
    {
      path: '/:pathMatch(.*)*',
      name: 'not-found',
      component: () => import('./views/NotFoundView.vue'),
      meta: { public: true, title: 'Page not found' },
    },
  ],
})

router.afterEach((to) => {
  updatePageMetadata(to.path, to.meta.title ? `${String(to.meta.title)} | rakkio` : 'rakkio')
})
