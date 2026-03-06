import { invoke } from '@tauri-apps/api/core'
import type { PlatformListing, FullListing, Platform, EbayCategorySuggestion, EbayAspectMetadata, EbayBusinessPolicy } from '~/types'

export function useListings() {
  const listings = ref<PlatformListing[]>([])
  const loading = ref(false)

  async function fetchAllListings(platform: Platform) {
    loading.value = true
    try {
      listings.value = await invoke<PlatformListing[]>('fetch_all_listings', { platform })
    } finally {
      loading.value = false
    }
  }

  async function fetchUnmappedListings(platform: Platform): Promise<PlatformListing[]> {
    return invoke<PlatformListing[]>('fetch_unmapped_listings', { platform })
  }

  async function diffListings(productId: string): Promise<FullListing[]> {
    return invoke<FullListing[]>('diff_listings', { productId })
  }

  async function migrateListing(
    sourcePlatform: Platform,
    sourceItemId: string,
    targetPlatform: Platform,
    productId: string,
    variantId: string,
  ): Promise<string> {
    return invoke<string>('migrate_listing', {
      sourcePlatform,
      sourceItemId,
      targetPlatform,
      productId,
      variantId,
    })
  }

  async function createListingOnPlatform(params: {
    targetPlatform: Platform
    productId: string
    title: string
    descriptionHtml: string | null
    priceAmount: number | null
    priceCurrency: string | null
    photoUrls: string[]
    extras?: Record<string, string>
    variantId: string
    sku?: string | null
    quantity?: number | null
  }): Promise<string> {
    return invoke<string>('create_listing_on_platform', params)
  }

  async function updateListingOnPlatform(params: {
    platform: Platform
    platformItemId: string
    title?: string
    descriptionHtml?: string | null
    priceAmount?: number | null
    priceCurrency?: string | null
    tags?: string[]
    extras?: Record<string, string>
  }): Promise<void> {
    return invoke('update_listing_on_platform', params)
  }

  async function ebayGetCategorySuggestions(query: string, marketplaceId?: string): Promise<EbayCategorySuggestion[]> {
    return invoke<EbayCategorySuggestion[]>('ebay_get_category_suggestions', { query, marketplaceId })
  }

  async function ebayGetCategoryAspects(categoryId: string, marketplaceId?: string): Promise<EbayAspectMetadata[]> {
    return invoke<EbayAspectMetadata[]>('ebay_get_category_aspects', { categoryId, marketplaceId })
  }

  async function ebayGetBusinessPolicies(marketplaceId?: string): Promise<[EbayBusinessPolicy[], EbayBusinessPolicy[], EbayBusinessPolicy[]]> {
    return invoke<[EbayBusinessPolicy[], EbayBusinessPolicy[], EbayBusinessPolicy[]]>('ebay_get_business_policies', { marketplaceId })
  }

  return {
    listings,
    loading,
    fetchAllListings,
    fetchUnmappedListings,
    diffListings,
    migrateListing,
    createListingOnPlatform,
    updateListingOnPlatform,
    ebayGetCategorySuggestions,
    ebayGetCategoryAspects,
    ebayGetBusinessPolicies,
  }
}
