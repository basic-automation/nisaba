<script setup lang="ts">
import type { NotificationType } from '~/types'

const { notifications, unreadCount, markAllRead, markRead, clearAll, remove } = useNotifications()

const typeFilter = ref<NotificationType | 'all'>('all')

const typeFilters: { value: NotificationType | 'all'; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'error', label: 'Errors' },
  { value: 'warning', label: 'Warnings' },
  { value: 'info', label: 'Info' },
  { value: 'success', label: 'Success' },
]

const filtered = computed(() => {
  if (typeFilter.value === 'all') return notifications.value
  return notifications.value.filter(n => n.type === typeFilter.value)
})

function formatTimestamp(ts: number): string {
  const diff = Date.now() - ts
  if (diff < 60_000) return 'Just now'
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`
  if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`
  const d = new Date(ts)
  return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' })
}

function dotClass(type: NotificationType): string {
  switch (type) {
    case 'success': return 'bg-green-400'
    case 'error': return 'bg-red-400'
    case 'warning': return 'bg-yellow-400'
    case 'info': return 'bg-blue-400'
  }
}

const copiedId = ref<string | null>(null)

function toMarkdown(n: typeof notifications.value[0]): string {
  const ts = new Date(n.timestamp).toLocaleString()
  const typeLabel = n.type.toUpperCase()
  let md = `## ${typeLabel}: ${n.title}\n\n`
  md += `**Time:** ${ts}\n`
  if (n.source) md += `**Source:** ${n.source}\n`
  md += `\n${n.message}\n`
  if (n.detail) md += `\n\`\`\`\n${n.detail}\n\`\`\`\n`
  return md
}

async function copyAsMarkdown(n: typeof notifications.value[0]) {
  try {
    await navigator.clipboard.writeText(toMarkdown(n))
    copiedId.value = n.id
    setTimeout(() => { copiedId.value = null }, 2000)
  } catch {
    // Fallback for older browsers
    const ta = document.createElement('textarea')
    ta.value = toMarkdown(n)
    document.body.appendChild(ta)
    ta.select()
    document.execCommand('copy')
    document.body.removeChild(ta)
    copiedId.value = n.id
    setTimeout(() => { copiedId.value = null }, 2000)
  }
}
</script>

<template>
  <div class="flex flex-col h-full">
    <!-- Header -->
    <div class="flex items-end justify-between mb-10 shrink-0">
      <h2 class="text-sm font-medium tracking-widest uppercase text-muted/60">Notifications</h2>
      <div class="flex items-center gap-3">
        <Button v-if="unreadCount > 0" variant="ghost" color="accent" size="xs" @click="markAllRead">
          Mark all read
        </Button>
        <Button v-if="notifications.length > 0" variant="ghost" color="muted" size="xs" @click="clearAll">
          Clear all
        </Button>
      </div>
    </div>

    <!-- Filters -->
    <div class="flex items-center gap-3 mb-6 shrink-0">
      <Button
        v-for="f in typeFilters"
        :key="f.value"
        variant="solid"
        :color="typeFilter === f.value ? 'accent' : 'muted'"
        size="xs"
        @click="typeFilter = f.value"
      >
        {{ f.label }}
      </Button>
    </div>

    <!-- Status line -->
    <div class="flex items-center gap-3 text-[11px] text-muted/30 mb-6 shrink-0">
      <span>{{ filtered.length }} notification{{ filtered.length !== 1 ? 's' : '' }}</span>
      <span v-if="unreadCount > 0">&middot; {{ unreadCount }} unread</span>
    </div>

    <!-- Notification list -->
    <div class="flex-1 overflow-auto min-h-0 -mr-6 pr-6 space-y-2">
      <TransitionGroup name="notif-list">
        <GroundGlass
          v-for="n in filtered"
          :key="n.id"
          class="notification-card p-4"
          :class="{ 'notification-card--unread': !n.read }"
          @click="markRead(n.id)"
        >
          <div class="relative z-[1] flex items-start gap-3">
            <!-- Type dot -->
            <span class="w-2 h-2 rounded-full mt-1.5 shrink-0" :class="dotClass(n.type)" />

            <div class="flex-1 min-w-0">
              <!-- Title + timestamp -->
              <div class="flex items-center justify-between gap-4 mb-1">
                <h4 class="text-sm font-medium text-foreground/90 truncate">{{ n.title }}</h4>
                <span class="text-[11px] text-muted/30 whitespace-nowrap shrink-0">
                  {{ formatTimestamp(n.timestamp) }}
                </span>
              </div>

              <!-- Message -->
              <p class="text-xs text-foreground/60 leading-relaxed">{{ n.message }}</p>

              <!-- Detail (rich, monospace for stack traces etc.) -->
              <div v-if="n.detail" class="mt-2 p-2.5 rounded-md bg-black/15 border border-border/10">
                <p class="text-[11px] text-muted/45 font-mono whitespace-pre-wrap break-all leading-relaxed">{{ n.detail }}</p>
              </div>

              <!-- Source badge -->
              <span v-if="n.source" class="inline-block text-[10px] text-muted/25 mt-2 uppercase tracking-wider">
                {{ n.source }}
              </span>
            </div>

            <!-- Actions -->
            <div class="flex flex-col gap-1.5 shrink-0 mt-0.5">
              <!-- Copy as markdown -->
              <button
                class="text-muted/20 hover:text-muted/50 transition-colors"
                :class="{ 'text-green-400/70': copiedId === n.id }"
                title="Copy as Markdown"
                @click.stop="copyAsMarkdown(n)"
              >
                <svg v-if="copiedId !== n.id" class="w-3.5 h-3.5" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.8">
                  <rect x="6" y="6" width="10" height="10" rx="1.5" />
                  <path d="M14 6V4.5A1.5 1.5 0 0 0 12.5 3h-8A1.5 1.5 0 0 0 3 4.5v8A1.5 1.5 0 0 0 4.5 14H6" />
                </svg>
                <svg v-else class="w-3.5 h-3.5" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M5 10l3.5 3.5L15 7" />
                </svg>
              </button>
              <!-- Dismiss -->
              <button
                class="text-muted/20 hover:text-muted/50 transition-colors"
                title="Remove"
                @click.stop="remove(n.id)"
              >
                <svg class="w-3.5 h-3.5" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.8">
                  <path d="M6 6l8 8M14 6l-8 8" />
                </svg>
              </button>
            </div>
          </div>
        </GroundGlass>
      </TransitionGroup>

      <!-- Empty state -->
      <div v-if="filtered.length === 0" class="py-16 text-sm text-muted/40 text-center">
        {{ notifications.length === 0 ? 'No notifications yet.' : 'No matching notifications.' }}
      </div>
    </div>
  </div>
</template>

<style scoped>
.notification-card {
  cursor: pointer;
  transition: border-color 0.2s ease;
}

.notification-card--unread {
  border-left: 2px solid rgba(139, 92, 246, 0.5);
}

.notif-list-enter-active {
  transition: all 0.25s ease;
}
.notif-list-leave-active {
  transition: all 0.2s ease;
}
.notif-list-enter-from {
  opacity: 0;
  transform: translateY(-8px);
}
.notif-list-leave-to {
  opacity: 0;
  transform: translateX(20px);
}
.notif-list-move {
  transition: transform 0.25s ease;
}
</style>
