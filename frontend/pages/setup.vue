<script setup lang="ts">
const route = useRoute()

// N-2.7: the existing-company platform-setup flow is hosted in the dedicated
// settings window (the only context allowed to read platform secrets), so
// finishing there closes the window instead of navigating the main window.
const inSettings = isSettingsWindow()

// Query params for configuring an existing company (skips step 1)
const existingId = (route.query.companyId as string) || undefined
const existingName = (route.query.companyName as string) || undefined
// N-11.2: credential-health warnings deep-link here with ?platform=ebay so
// the wizard opens straight on the step that renews the credential.
const initialPlatform = (route.query.platform as string) || undefined

async function onComplete() {
  if (inSettings) {
    await closeCurrentWindow()
    return
  }
  navigateTo('/')
}

async function onCancel() {
  if (inSettings) {
    await closeCurrentWindow()
    return
  }
  navigateTo('/companies')
}
</script>

<template>
  <div class="flex flex-col h-full">
    <div class="shrink-0 pb-8 max-w-2xl">
      <h2 class="text-sm font-medium tracking-widest uppercase text-muted/60">
        {{ existingId ? 'Configure Platforms' : 'New Company' }}
      </h2>
    </div>

    <div class="flex-1 overflow-auto min-h-0 -mr-6 pr-6">
      <SetupWizard
        :existing-company-id="existingId"
        :existing-company-name="existingName"
        :initial-platform="initialPlatform"
        @complete="onComplete"
        @cancel="onCancel"
      />
    </div>
  </div>
</template>
