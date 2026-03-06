import { invoke } from '@tauri-apps/api/core'
import type { ProductAnalytics, TimeWindow } from '~/types'

export function useAnalytics() {
  const analytics = ref<ProductAnalytics[]>([])
  const loading = ref(false)

  async function fetchAllAnalytics(window: TimeWindow = 'Week') {
    loading.value = true
    try {
      analytics.value = await invoke<ProductAnalytics[]>('get_all_analytics', { window })
    } finally {
      loading.value = false
    }
  }

  async function fetchProductAnalytics(productId: string, window: TimeWindow = 'Week'): Promise<ProductAnalytics> {
    return invoke<ProductAnalytics>('get_product_analytics', { productId, window })
  }

  return {
    analytics,
    loading,
    fetchAllAnalytics,
    fetchProductAnalytics,
  }
}
