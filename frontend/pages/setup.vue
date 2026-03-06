<script setup lang="ts">
const route = useRoute()

// Query params for configuring an existing company (skips step 1)
const existingId = (route.query.companyId as string) || undefined
const existingName = (route.query.companyName as string) || undefined

function onComplete() {
  navigateTo('/')
}

function onCancel() {
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
        @complete="onComplete"
        @cancel="onCancel"
      />
    </div>
  </div>
</template>
