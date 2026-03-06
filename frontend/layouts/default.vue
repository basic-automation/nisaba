<script setup lang="ts">
const { fetchCompanies } = useCompanyContext()
const { unreadCount } = useNotifications()

const navItems = [
  { label: 'Dashboard', path: '/', icon: 'dashboard' },
  { label: 'Products', path: '/products', icon: 'package' },
  { label: 'Listings', path: '/listings', icon: 'listings' },
  { label: 'Vendors', path: '/vendors', icon: 'vendor' },
  { label: 'Marketplace', path: '/marketplace', icon: 'marketplace' },
  { label: 'Inventory', path: '/inventory', icon: 'chart' },
  { label: 'Companies', path: '/companies', icon: 'company' },
  { label: 'Config', path: '/config', icon: 'gear' },
  { label: 'Notifications', path: '/notifications', icon: 'bell' },
  { label: 'Logs', path: '/logs', icon: 'document' },
]

const route = useRoute()

function isActive(item: { path: string }) {
  return route.path === item.path || (item.path !== '/' && route.path.startsWith(item.path))
}

onMounted(() => {
  fetchCompanies()
})
</script>

<template>
  <div class="app-shell flex h-screen overflow-hidden">
    <!-- Activity Bar -->
    <aside class="flex-shrink-0 flex flex-col items-center">
      <!-- Brand logo -->
      <div class="flex items-center justify-center pt-2 pb-4 pl-3 pr-6">
        <img src="/logo.svg" alt="Nisaba" class="w-16 h-16 select-none" />
      </div>

      <!-- Navigation -->
      <nav class="flex flex-col items-start gap-1 mt-1 self-start pl-4">
        <NuxtLink
          v-for="item in navItems"
          :key="item.path"
          :to="item.path"
          class="activity-icon group relative flex items-center justify-center w-full h-10 pl-2"
          :class="[
            isActive(item)
              ? 'active text-foreground'
              : 'text-muted/40 hover:text-muted'
          ]"
        >
          <!-- Active indicator -->
          <span
            v-if="isActive(item)"
            class="absolute left-0 top-1 bottom-1 w-0.5 bg-accent rounded-r"
          />

          <!-- Dashboard: 4-square grid -->
          <svg v-if="item.icon === 'dashboard'" class="w-5 h-5" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5">
            <rect x="2" y="2" width="7" height="7" rx="1" />
            <rect x="11" y="2" width="7" height="7" rx="1" />
            <rect x="2" y="11" width="7" height="7" rx="1" />
            <rect x="11" y="11" width="7" height="7" rx="1" />
          </svg>

          <!-- Products: Box/package -->
          <svg v-else-if="item.icon === 'package'" class="w-5 h-5" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5">
            <path d="M10 2 L18 6 L18 14 L10 18 L2 14 L2 6 Z" />
            <path d="M10 18 L10 10" />
            <path d="M18 6 L10 10 L2 6" />
            <path d="M6 4 L14 8" />
          </svg>

          <!-- Listings: Broadcast/external -->
          <svg v-else-if="item.icon === 'listings'" class="w-5 h-5" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5">
            <path d="M3 4h14M3 8h10M3 12h14M3 16h10" />
          </svg>

          <!-- Vendors: Storefront/truck -->
          <svg v-else-if="item.icon === 'vendor'" class="w-5 h-5" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5">
            <path d="M2 9h16M2 9l1-5h14l1 5M2 9v8h16V9" />
            <path d="M8 17v-4h4v4" />
            <path d="M5 9v2M10 9v2M15 9v2" />
          </svg>

          <!-- Marketplace: Shopping bag / store -->
          <svg v-else-if="item.icon === 'marketplace'" class="w-5 h-5" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5">
            <path d="M4 7h12l-1 10H5L4 7z" />
            <path d="M8 7V5a2 2 0 0 1 4 0v2" />
            <circle cx="8" cy="12" r="1" fill="currentColor" stroke="none" />
            <circle cx="12" cy="12" r="1" fill="currentColor" stroke="none" />
          </svg>

          <!-- Inventory: Bar chart -->
          <svg v-else-if="item.icon === 'chart'" class="w-5 h-5" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5">
            <rect x="2" y="10" width="3.5" height="8" rx="0.5" />
            <rect x="8.25" y="5" width="3.5" height="13" rx="0.5" />
            <rect x="14.5" y="2" width="3.5" height="16" rx="0.5" />
          </svg>

          <!-- Company: People/network -->
          <svg v-else-if="item.icon === 'company'" class="w-5 h-5" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5">
            <circle cx="10" cy="5" r="2.5" />
            <circle cx="4" cy="14" r="2" />
            <circle cx="16" cy="14" r="2" />
            <path d="M10 7.5v3M7 13l2-2.5M13 13l-2-2.5" />
          </svg>

          <!-- Config: Gear/cog -->
          <svg v-else-if="item.icon === 'gear'" class="w-5 h-5" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5">
            <circle cx="10" cy="10" r="3" />
            <path d="M10 1.5v2M10 16.5v2M18.5 10h-2M3.5 10h-2M16 4l-1.4 1.4M5.4 14.6L4 16M16 16l-1.4-1.4M5.4 5.4L4 4" />
          </svg>

          <!-- Notifications: Bell -->
          <svg v-else-if="item.icon === 'bell'" class="w-5 h-5" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5">
            <path d="M10 2.5a5 5 0 0 0-5 5v3l-1.5 2.5h13L15 10.5v-3a5 5 0 0 0-5-5z" />
            <path d="M8 16a2 2 0 0 0 4 0" />
          </svg>

          <!-- Logs: Document/list -->
          <svg v-else-if="item.icon === 'document'" class="w-5 h-5" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5">
            <path d="M5 2h7l4 4v12a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1z" />
            <path d="M12 2v4h4" />
            <path d="M7 10h6M7 13h4" />
          </svg>

          <!-- Unread notification badge -->
          <span
            v-if="item.icon === 'bell' && unreadCount > 0"
            class="notif-badge"
          >{{ unreadCount > 99 ? '99+' : unreadCount }}</span>

          <!-- Tooltip -->
          <span class="tooltip">{{ item.label }}</span>
        </NuxtLink>
      </nav>

      <!-- Sync controls at bottom -->
      <div class="mt-auto mb-3">
        <StatusBar />
      </div>
    </aside>

    <!-- Main content column -->
    <div class="flex-1 flex flex-col min-h-0">
      <TitleBar />
      <UpdateBanner />
      <main class="flex-1 overflow-hidden p-6">
        <slot />
      </main>
      <PlatformStatusBar />
    </div>
    <ToastContainer />
  </div>
</template>

<style scoped>
/* ════ Grainy mesh gradient ════ */
.app-shell {
  position: relative;
  z-index: 0;
  background: #1e293b;
}
/* Mesh gradient blobs */
.app-shell::before {
  content: '';
  position: absolute;
  inset: 0;
  z-index: -2;
  pointer-events: none;
  background:
    radial-gradient(at 0% 0%, rgba(139, 92, 246, 0.35), transparent 50%),
    radial-gradient(at 100% 100%, rgba(34, 197, 94, 0.25), transparent 50%),
    radial-gradient(at 85% 5%, rgba(239, 68, 68, 0.20), transparent 40%),
    radial-gradient(at 8% 95%, rgba(251, 191, 36, 0.20), transparent 45%),
    radial-gradient(at 50% 50%, rgba(139, 92, 246, 0.08), transparent 60%);
}
/* Grain noise */
.app-shell::after {
  content: '';
  position: absolute;
  inset: 0;
  z-index: -1;
  pointer-events: none;
  opacity: 0.50;
  mix-blend-mode: overlay;
  background-image: url("data:image/svg+xml,%3Csvg%20xmlns='http://www.w3.org/2000/svg'%20width='200'%20height='200'%3E%3Cfilter%20id='n'%3E%3CfeTurbulence%20type='fractalNoise'%20baseFrequency='0.45'%20numOctaves='3'%20stitchTiles='stitch'/%3E%3C/filter%3E%3Crect%20width='100%25'%20height='100%25'%20filter='url(%23n)'/%3E%3C/svg%3E");
  background-repeat: repeat;
}

.activity-icon {
  transition: color 0.15s ease;
}

.tooltip {
  position: absolute;
  left: 100%;
  top: 50%;
  transform: translateY(-50%);
  margin-left: 8px;
  padding: 4px 10px;
  background: #1a1a2e;
  color: #e2e8f0;
  font-size: 12px;
  white-space: nowrap;
  border-radius: 4px;
  pointer-events: none;
  opacity: 0;
  transition: opacity 0.15s ease;
  z-index: 50;
}

.activity-icon:hover .tooltip {
  opacity: 1;
}

.notif-badge {
  position: absolute;
  top: 0;
  right: 2px;
  min-width: 16px;
  height: 16px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 9px;
  font-weight: 700;
  color: #fff;
  background: #8b5cf6;
  border-radius: 9999px;
  padding: 0 4px;
  line-height: 1;
  pointer-events: none;
}
</style>
