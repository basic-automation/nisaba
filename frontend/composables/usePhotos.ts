import { invoke } from '@tauri-apps/api/core'
import type { ListingPhoto, Platform } from '~/types'

export function usePhotos() {
  const photos = ref<[Platform, ListingPhoto][]>([])
  const loading = ref(false)

  async function fetchProductPhotos(productId: string, platform?: Platform) {
    loading.value = true
    try {
      photos.value = await invoke<[Platform, ListingPhoto][]>('get_product_photos', {
        productId,
        platform: platform ?? null,
      })
    } finally {
      loading.value = false
    }
  }

  async function syncPhoto(
    targetPlatform: Platform,
    targetItemId: string,
    photoUrl: string,
    position: number,
  ): Promise<string> {
    return invoke<string>('sync_photo', {
      targetPlatform,
      targetItemId,
      photoUrl,
      position,
    })
  }

  return {
    photos,
    loading,
    fetchProductPhotos,
    syncPhoto,
  }
}
