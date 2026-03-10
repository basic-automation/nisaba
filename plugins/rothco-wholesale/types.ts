// Rothco GraphQL API v2 response types

export interface PaginatorInfo {
  currentPage: number
  lastPage: number
  hasMorePages: boolean
  total: number
  perPage: number
}

export interface ProductLineListResponse {
  data: {
    productLine: {
      data: ProductLineListItem[]
      paginatorInfo: PaginatorInfo
    }
  }
}

export interface ProductLineListItem {
  product_line_id: number
  product_line_code: string
  product_line_name: string
}

export interface ProductLineDetailResponse {
  data: {
    productLineById: ProductLineDetail
  }
}

export interface ProductLineDetail {
  product_line_id: number
  product_line_code: string
  product_line_name: string
  short_description: string | null
  skus: RothcoSku[]
}

export interface RothcoSkuPrices {
  price: number | null
  case_price: number | null
  map_price: number | null
}

export interface RothcoSku {
  sku_code: string
  upc: string
  image: string | null
  weight: number | null
  prices: RothcoSkuPrices | null
  specifications: Specification[]
}

export interface Specification {
  spec_name: string
  value: string
}

export interface InventoryResponse {
  data: {
    inventory: {
      data: InventoryItem[]
      paginatorInfo: PaginatorInfo
    }
  }
}

export interface InventoryItem {
  upc: string
  inventory_level: string // API returns as string
}

export interface SkuImagesResponse {
  data: {
    skuImages: {
      data: SkuImageItem[]
      paginatorInfo: PaginatorInfo
    }
  }
}

export interface SkuImageItem {
  sku_code: string
  image_id: string
}
