<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'

// N-11.4: About tab of Settings. Shows the app version and hosts
// the one-button diagnostics export (the 1.0 replacement for telemetry /
// crash-reporting services).
const { notify } = useNotifications()

const appVersion = ref('')
const exporting = ref(false)
const lastResult = ref('')

onMounted(async () => {
  try {
    appVersion.value = await invoke<string>('get_app_version')
  } catch (e: any) {
    notify({ type: 'error', title: 'Failed to read app version', message: String(e), source: 'about' })
  }
})

// N-12.1: the in-app user guide ships as static HTML in
// frontend/public/guide/ and opens in a dedicated help window.
async function handleOpenGuide() {
  try {
    await openGuideWindow()
  } catch (e: any) {
    notify({ type: 'error', title: 'Could not open the user guide', message: String(e), source: 'about' })
  }
}

async function handleExportDiagnostics() {
  exporting.value = true
  lastResult.value = ''
  try {
    const result = await invoke<string>('export_diagnostics')
    if (result !== 'Cancelled') {
      lastResult.value = result
      notify({ type: 'success', title: 'Diagnostics exported', message: result, source: 'about' })
    }
  } catch (e: any) {
    notify({ type: 'error', title: 'Diagnostics export failed', message: String(e), detail: String(e), source: 'about' })
  } finally {
    exporting.value = false
  }
}
</script>

<template>
  <div class="max-w-2xl mx-auto space-y-6 overflow-y-auto h-full pr-1">
    <section class="rounded-xl border border-white/10 bg-white/[0.03] p-6">
      <h2 class="text-lg font-semibold text-foreground">About Nisaba</h2>
      <p class="mt-1 text-sm text-muted/70">Inventory Sync Manager</p>
      <dl class="mt-4 space-y-2 text-sm">
        <div class="flex gap-3">
          <dt class="w-28 shrink-0 text-muted/50">Version</dt>
          <dd class="text-foreground font-mono">{{ appVersion || '…' }}</dd>
        </div>
      </dl>
    </section>

    <section class="rounded-xl border border-white/10 bg-white/[0.03] p-6">
      <h2 class="text-lg font-semibold text-foreground">User guide</h2>
      <p class="mt-2 text-sm text-muted/70 leading-relaxed">
        The in-app guide covers setup, each platform's credentials, mappings
        and variants, what sync does and does not do, backups, troubleshooting,
        and the eBay account-deletion exemption walkthrough. It opens in its
        own window so you can keep working.
      </p>
      <button
        class="mt-5 px-4 py-2 rounded-lg bg-accent text-white text-sm font-medium hover:bg-accent/90 transition-colors"
        @click="handleOpenGuide"
      >
        Open user guide
      </button>
    </section>

    <section class="rounded-xl border border-white/10 bg-white/[0.03] p-6">
      <h2 class="text-lg font-semibold text-foreground">Third-party licenses</h2>
      <p class="mt-2 text-sm text-muted/70 leading-relaxed">
        Nisaba is built on open-source software. The full list of Rust and npm
        dependencies and their license notices — generated at build time by
        <span class="font-mono">cargo-about</span> and
        <span class="font-mono">license-checker</span>, and bundled with the
        app — lives in the Licenses view.
      </p>
      <button
        class="mt-5 px-4 py-2 rounded-lg bg-accent text-white text-sm font-medium hover:bg-accent/90 transition-colors"
        @click="navigateTo('/about/licenses')"
      >
        View licenses
      </button>
    </section>

    <section class="rounded-xl border border-white/10 bg-white/[0.03] p-6">
      <h2 class="text-lg font-semibold text-foreground">Diagnostics</h2>
      <p class="mt-2 text-sm text-muted/70 leading-relaxed">
        Nisaba sends no telemetry and has no crash-reporting service. If you hit
        a bug, export a diagnostics bundle and attach it to your report. The
        bundle is a zip containing:
      </p>
      <ul class="mt-3 space-y-1.5 text-sm text-muted/70 list-disc list-inside">
        <li>Recent application logs</li>
        <li>App, OS and platform versions</li>
        <li>The app configuration with <span class="font-mono text-foreground/80">all secrets redacted</span></li>
      </ul>
      <p class="mt-3 text-sm text-muted/70 leading-relaxed">
        Platform secrets, tokens and the company secret never appear in the
        bundle — secret values are replaced with
        <span class="font-mono text-foreground/80">[REDACTED]</span> before the
        zip is written.
      </p>
      <button
        :disabled="exporting"
        class="mt-5 px-4 py-2 rounded-lg bg-accent text-white text-sm font-medium hover:bg-accent/90 disabled:opacity-50 disabled:cursor-wait transition-colors"
        @click="handleExportDiagnostics"
      >
        {{ exporting ? 'Exporting…' : 'Export diagnostics' }}
      </button>
      <p v-if="lastResult" class="mt-3 text-xs text-muted/60">{{ lastResult }}</p>
    </section>
  </div>
</template>
