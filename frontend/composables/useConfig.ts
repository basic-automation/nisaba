import { invoke } from '@tauri-apps/api/core'
import type { AppConfig, PlatformCapabilities } from '~/types'

export function useConfig() {
  const config = ref<AppConfig | null>(null)
  const capabilities = ref<Record<string, PlatformCapabilities>>({})
  const loading = ref(false)

  async function fetchConfig() {
    loading.value = true
    try {
      config.value = await invoke<AppConfig>('get_config')
    } finally {
      loading.value = false
    }
  }

  async function saveConfig(updated: AppConfig) {
    await invoke('update_config', { config: updated })
    config.value = updated
  }

  async function fetchCapabilities() {
    capabilities.value = await invoke<Record<string, PlatformCapabilities>>('get_platform_capabilities')
    return capabilities.value
  }

  async function fetchCompanyConfig(): Promise<Record<string, any> | null> {
    try {
      return await invoke<Record<string, any>>('get_company_platform_config')
    } catch {
      return null
    }
  }

  async function saveCompanyConfig(config: Record<string, any>) {
    await invoke('update_company_platform_config', { config })
  }

  return {
    config,
    capabilities,
    loading,
    fetchConfig,
    saveConfig,
    fetchCapabilities,
    fetchCompanyConfig,
    saveCompanyConfig,
  }
}
