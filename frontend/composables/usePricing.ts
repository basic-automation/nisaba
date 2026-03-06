import { invoke } from '@tauri-apps/api/core'
import type { PricingSnapshot, Platform } from '~/types'

export function usePricing() {
  const prices = ref<PricingSnapshot[]>([])
  const history = ref<PricingSnapshot[]>([])
  const loading = ref(false)

  async function fetchProductPrices(productId: string) {
    loading.value = true
    try {
      prices.value = await invoke<PricingSnapshot[]>('get_product_prices', { productId })
    } finally {
      loading.value = false
    }
  }

  async function updatePrice(
    platform: Platform,
    platformItemId: string,
    amount: number,
    currency: string,
  ) {
    await invoke('update_price', { platform, platformItemId, amount, currency })
  }

  async function fetchPricingHistory(productId: string, interval?: string) {
    history.value = await invoke<PricingSnapshot[]>('get_pricing_history', {
      productId,
      interval: interval ?? null,
    })
  }

  return {
    prices,
    history,
    loading,
    fetchProductPrices,
    updatePrice,
    fetchPricingHistory,
  }
}
