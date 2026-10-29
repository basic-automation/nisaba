<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import type {
  ThirdPartyLicenses,
  ThirdPartyLicenseText,
  NpmLicenseText,
} from '~/types'

// N-12.3: About -> Licenses view. Lists every third-party dependency and its
// license, generated at build time by `cargo-about` (Rust) and
// `license-checker` (npm) and bundled in the app resources. Most dependencies
// are MIT or Apache-2.0, and both licenses require shipping their notices
// with the binaries — this view is where those notices live.
const { notify } = useNotifications()

const licenses = ref<ThirdPartyLicenses | null>(null)
const loadError = ref('')
const rustQuery = ref('')
const npmQuery = ref('')

onMounted(async () => {
  try {
    licenses.value = await invoke<ThirdPartyLicenses>('get_third_party_licenses')
  } catch (e: any) {
    loadError.value = String(e)
    notify({ type: 'error', title: 'Failed to load license notices', message: String(e), source: 'licenses' })
  }
})

function matches(query: string, ...fields: (string | null | undefined)[]): boolean {
  const q = query.trim().toLowerCase()
  if (!q) return true
  return fields.some((f) => (f ?? '').toLowerCase().includes(q))
}

const filteredRustDeps = computed(() => {
  const deps = licenses.value?.rust.dependencies ?? []
  return deps.filter((d) => matches(rustQuery.value, d.name, d.version, d.license))
})

const filteredNpmPackages = computed(() => {
  const pkgs = licenses.value?.npm.packages ?? []
  return pkgs.filter((p) => matches(npmQuery.value, p.name, p.version, ...p.licenses))
})

// Full license texts applying to one Rust dependency, resolved exactly via
// the `used_by` mapping cargo-about produced (no SPDX expression guessing).
function rustTextsFor(name: string, version: string): ThirdPartyLicenseText[] {
  const all = licenses.value?.rust.licenses ?? []
  return all.filter((t) => t.crates.some((c) => c.name === name && c.version === version))
}

// Full license texts applying to one npm package, resolved via the
// deduplicated `license_texts` mapping the generate script produced.
function npmTextsFor(name: string, version: string): NpmLicenseText[] {
  const key = `${name}@${version}`
  return (licenses.value?.npm.license_texts ?? []).filter((t) => t.packages.includes(key))
}
</script>

<template>
  <div class="max-w-3xl mx-auto space-y-6 overflow-y-auto h-full pr-1 pb-8">
    <section class="rounded-xl border border-white/10 bg-white/[0.03] p-6">
      <button
        class="text-xs text-accent hover:underline"
        @click="navigateTo('/settings?tab=about')"
      >
        ← Back to About
      </button>
      <h2 class="mt-2 text-lg font-semibold text-foreground">Third-party licenses</h2>
      <p class="mt-2 text-sm text-muted/70 leading-relaxed">
        Nisaba is built on open-source software. The notices below are
        generated at build time — <span class="font-mono">cargo-about</span>
        for the Rust dependencies and
        <span class="font-mono">license-checker</span> for the npm packages —
        and bundled with the app, because licenses like MIT and Apache-2.0
        require their notices to ship with the binaries.
      </p>
      <p v-if="licenses" class="mt-3 text-xs text-muted/50 font-mono">
        {{ licenses.rust.generated_by }} · {{ licenses.npm.generated_by }} ·
        {{ licenses.rust.dependencies.length }} Rust crates ·
        {{ licenses.npm.packages.length }} npm packages
      </p>
      <p v-else-if="loadError" class="mt-3 text-sm text-red-400">{{ loadError }}</p>
      <p v-else class="mt-3 text-sm text-muted/50">Loading notices…</p>
    </section>

    <section v-if="licenses" class="rounded-xl border border-white/10 bg-white/[0.03] p-6">
      <div class="flex items-center justify-between gap-4 flex-wrap">
        <h3 class="text-md font-semibold text-foreground">Rust dependencies</h3>
        <input
          v-model="rustQuery"
          type="search"
          placeholder="Filter by name or license…"
          class="px-3 py-1.5 rounded-lg bg-black/30 border border-white/10 text-sm text-foreground placeholder:text-muted/40 focus:outline-none focus:border-accent/60 w-64"
        />
      </div>
      <p class="mt-1 text-xs text-muted/50">
        Showing {{ filteredRustDeps.length }} of {{ licenses.rust.dependencies.length }}
      </p>
      <div class="mt-4 space-y-1.5 max-h-[28rem] overflow-y-auto pr-1">
        <details
          v-for="dep in filteredRustDeps"
          :key="`rust-${dep.name}-${dep.version}`"
          class="rounded-lg border border-white/5 bg-black/20 px-3 py-2"
        >
          <summary class="cursor-pointer text-sm flex items-baseline gap-2 flex-wrap">
            <span class="font-medium text-foreground">{{ dep.name }}</span>
            <span class="font-mono text-xs text-muted/50">{{ dep.version }}</span>
            <span class="ml-auto font-mono text-xs text-accent/90">{{ dep.license }}</span>
          </summary>
          <div class="mt-2 space-y-3">
            <p v-if="dep.repository" class="text-xs text-muted/50 break-all">{{ dep.repository }}</p>
            <div v-for="t in rustTextsFor(dep.name, dep.version)" :key="t.id" class="rounded-md bg-black/30 p-3">
              <p class="text-xs font-semibold text-foreground">{{ t.name }}</p>
              <pre class="mt-1 max-h-56 overflow-y-auto whitespace-pre-wrap text-xs text-muted/70 leading-relaxed">{{ t.text }}</pre>
            </div>
          </div>
        </details>
      </div>
    </section>

    <section v-if="licenses" class="rounded-xl border border-white/10 bg-white/[0.03] p-6">
      <div class="flex items-center justify-between gap-4 flex-wrap">
        <h3 class="text-md font-semibold text-foreground">npm packages</h3>
        <input
          v-model="npmQuery"
          type="search"
          placeholder="Filter by name or license…"
          class="px-3 py-1.5 rounded-lg bg-black/30 border border-white/10 text-sm text-foreground placeholder:text-muted/40 focus:outline-none focus:border-accent/60 w-64"
        />
      </div>
      <p class="mt-1 text-xs text-muted/50">
        Showing {{ filteredNpmPackages.length }} of {{ licenses.npm.packages.length }}
      </p>
      <div class="mt-4 space-y-1.5 max-h-[28rem] overflow-y-auto pr-1">
        <details
          v-for="pkg in filteredNpmPackages"
          :key="`npm-${pkg.name}-${pkg.version}`"
          class="rounded-lg border border-white/5 bg-black/20 px-3 py-2"
        >
          <summary class="cursor-pointer text-sm flex items-baseline gap-2 flex-wrap">
            <span class="font-medium text-foreground">{{ pkg.name }}</span>
            <span class="font-mono text-xs text-muted/50">{{ pkg.version }}</span>
            <span class="ml-auto font-mono text-xs text-accent/90">{{ pkg.licenses.join(', ') || 'unknown' }}</span>
          </summary>
          <div class="mt-2 space-y-3">
            <p v-if="pkg.repository" class="text-xs text-muted/50 break-all">{{ pkg.repository }}</p>
            <div v-for="t in npmTextsFor(pkg.name, pkg.version)" :key="t.licenses + t.text.slice(0, 32)" class="rounded-md bg-black/30 p-3">
              <p class="text-xs font-semibold text-foreground">{{ t.licenses }}</p>
              <pre class="mt-1 max-h-56 overflow-y-auto whitespace-pre-wrap text-xs text-muted/70 leading-relaxed">{{ t.text }}</pre>
            </div>
          </div>
        </details>
      </div>
    </section>
  </div>
</template>
