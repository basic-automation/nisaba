import { invoke } from '@tauri-apps/api/core'
import type { Product, PlatformMapping, PlatformListing, ProductVariant } from '~/types'

export function useProducts() {
  const products = ref<Product[]>([])
  const loading = ref(false)

  async function fetchProducts() {
    loading.value = true
    try {
      products.value = await invoke<Product[]>('list_products')
    } finally {
      loading.value = false
    }
  }

  async function getProduct(id: string): Promise<Product | null> {
    return invoke<Product | null>('get_product', { id })
  }

  async function createProduct(name: string, sku: string, quantity: number, lowStockThreshold?: number, withDefaultVariant = true): Promise<Product> {
    const product = await invoke<Product>('create_product', {
      name, sku, quantity, lowStockThreshold: lowStockThreshold ?? null, withDefaultVariant,
    })
    await fetchProducts()
    return product
  }

  async function updateProduct(id: string, name: string, sku: string, lowStockThreshold?: number) {
    await invoke('update_product', { id, name, sku, lowStockThreshold: lowStockThreshold ?? null })
    await fetchProducts()
  }

  async function deleteProduct(id: string) {
    await invoke('delete_product', { id })
    await fetchProducts()
  }

  async function getMappings(productId: string): Promise<PlatformMapping[]> {
    return invoke<PlatformMapping[]>('get_mappings', { productId })
  }

  // N-11.7: batch — one IPC round trip for many products (replaces the
  // per-product get_mappings loop on dashboard/listings/products pages).
  async function getMappingsBatch(productIds: string[]): Promise<Record<string, PlatformMapping[]>> {
    return invoke<Record<string, PlatformMapping[]>>('get_mappings_batch', { productIds })
  }

  async function createMapping(productId: string, platform: string, platformItemId: string, variantId: string, platformSku?: string) {
    await invoke('create_mapping', { productId, platform, platformItemId, platformSku: platformSku ?? null, variantId })
  }

  async function deleteMapping(id: number) {
    await invoke('delete_mapping', { id })
  }

  // ── Variants ──

  async function listVariants(productId: string): Promise<ProductVariant[]> {
    return invoke<ProductVariant[]>('list_variants', { productId })
  }

  // N-11.7: batch — one IPC round trip for many products (replaces the
  // per-product list_variants loop on the products page).
  async function listVariantsBatch(productIds: string[]): Promise<Record<string, ProductVariant[]>> {
    return invoke<Record<string, ProductVariant[]>>('list_variants_batch', { productIds })
  }

  async function createVariant(productId: string, sku: string, name: string, attributes: Record<string, string>, quantity: number, imageUrl?: string): Promise<ProductVariant> {
    return invoke<ProductVariant>('create_variant', {
      productId, sku, name, attributes, quantity, imageUrl: imageUrl ?? null,
    })
  }

  async function updateVariant(id: string, sku: string, name: string, attributes: Record<string, string>, quantity: number, imageUrl?: string) {
    await invoke('update_variant', { id, sku, name, attributes, quantity, imageUrl: imageUrl ?? null })
  }

  async function deleteVariant(id: string) {
    await invoke('delete_variant', { id })
  }

  return {
    products,
    loading,
    fetchProducts,
    getProduct,
    createProduct,
    updateProduct,
    deleteProduct,
    getMappings,
    getMappingsBatch,
    createMapping,
    deleteMapping,
    listVariants,
    listVariantsBatch,
    createVariant,
    updateVariant,
    deleteVariant,
  }
}
