import type { Config } from 'tailwindcss'

export default {
  darkMode: 'class',
  content: [
    './components/**/*.{js,vue,ts}',
    './layouts/**/*.vue',
    './pages/**/*.vue',
    './plugins/**/*.{js,ts}',
    './app.vue',
  ],
  theme: {
    extend: {
      fontFamily: {
        sans: ['"clother"', 'sans-serif'],
        display: ['"pulpo-rust-50"', 'serif'],
      },
      colors: {
        // Palenight-inspired color scheme
        background: '#1e293b',   // slate-800
        surface: '#334155',      // slate-700
        sidebar: '#0f172a',      // slate-900
        foreground: '#e2e8f0',   // slate-200
        muted: '#c4cdd6',        // lifted for gradient bg readability
        accent: '#8b5cf6',       // violet-500
        border: '#475569',       // slate-600
        error: '#ef4444',        // red-500
        warning: '#fbbf24',      // amber-400
        info: '#60a5fa',         // blue-400
      },
    },
  },
  plugins: [require('tailwindcss-animate')],
} satisfies Config
