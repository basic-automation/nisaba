export default defineNuxtConfig({
  compatibilityDate: '2025-01-01',
  ssr: false, // SPA mode for Tauri
  devtools: { enabled: false },

  experimental: {
    appManifest: false,
  },

  css: [
    '~/assets/css/fonts.css',
  ],

  modules: [
    '@nuxtjs/tailwindcss',
  ],

  components: [
    { path: '~/components/ui', pathPrefix: false },
    { path: '~/components/shared', pathPrefix: false },
    { path: '~/components/layout', pathPrefix: false },
    { path: '~/components', pathPrefix: false, ignore: ['ui/**', 'shared/**', 'layout/**'] },
  ],

  app: {
    head: {
      title: 'Nisaba',
      meta: [
        { name: 'description', content: 'Inventory Sync Manager' },
      ],
      link: [
        { rel: 'icon', type: 'image/png', href: '/icon.png' },
      ],
    },
  },

  // Tauri dev server
  devServer: {
    port: 3456,
  },

  // Vite config for Tauri compatibility
  vite: {
    clearScreen: false,
    envPrefix: ['VITE_', 'TAURI_'],
    optimizeDeps: {
      include: [
        '@tiptap/vue-3',
        '@tiptap/starter-kit',
        '@tiptap/extension-link',
        '@tiptap/extension-placeholder',
        '@tiptap/pm/state',
        '@tiptap/pm/view',
        '@tiptap/pm/model',
        '@tiptap/pm/transform',
      ],
    },
    server: {
      strictPort: true,
      hmr: {
        protocol: 'ws',
        host: 'localhost',
        port: 3457,
      },
    },
  },
})
