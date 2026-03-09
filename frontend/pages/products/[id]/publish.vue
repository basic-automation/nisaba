<script setup lang="ts">
import type { FullListing, Platform, PlatformCapabilities, PlatformMapping, VendorListingForProduct, ProductVariant, EbayCategorySuggestion, EbayAspectMetadata, EbayBusinessPolicy } from '~/types'
import { invoke } from '@tauri-apps/api/core'

const { resolveThumb } = useImageCache()

const route = useRoute()
const productId = route.params.id as string

const { getProduct, getMappings, listVariants } = useProducts()
const { diffListings, createListingOnPlatform, updateListingOnPlatform, ebayGetCategorySuggestions, ebayGetCategoryAspects, ebayGetBusinessPolicies } = useListings()
const { config, capabilities: platformCapabilities, fetchCapabilities, fetchConfig } = useConfig()

const platformLabels: Record<Platform, string> = {
  ebay: 'eBay',
  squarespace: 'Squarespace',
  xmrbazaar: 'XMR Bazaar',
  amazon: 'Amazon',
}

// --- eBay-specific state ---
const ebayCategoryId = ref('')
const ebayCategoryName = ref('')
const ebayCategorySuggestions = ref<EbayCategorySuggestion[]>([])
const ebayCategoryQuery = ref('')
const ebayCategoryLoading = ref(false)
const ebayCondition = ref('NEW')
const ebayConditionDescription = ref('')
const ebayAspects = ref<Record<string, string[]>>({})
const ebayAspectMetadata = ref<EbayAspectMetadata[]>([])
const ebayAspectsLoading = ref(false)
const ebayFormat = ref('FIXED_PRICE')
const ebayFulfillmentPolicyId = ref('')
const ebayPaymentPolicyId = ref('')
const ebayReturnPolicyId = ref('')
const ebayFulfillmentPolicies = ref<EbayBusinessPolicy[]>([])
const ebayPaymentPolicies = ref<EbayBusinessPolicy[]>([])
const ebayReturnPolicies = ref<EbayBusinessPolicy[]>([])
const ebayPoliciesLoading = ref(false)
const ebayPoliciesError = ref('')

const isEbay = computed(() => selectedPlatform.value === 'ebay')

const ebayConditions = [
  { value: 'NEW', label: 'New' },
  { value: 'LIKE_NEW', label: 'Like New' },
  { value: 'NEW_OTHER', label: 'New (Other)' },
  { value: 'MANUFACTURER_REFURBISHED', label: 'Manufacturer Refurbished' },
  { value: 'SELLER_REFURBISHED', label: 'Seller Refurbished' },
  { value: 'USED_EXCELLENT', label: 'Used - Excellent' },
  { value: 'USED_VERY_GOOD', label: 'Used - Very Good' },
  { value: 'USED_GOOD', label: 'Used - Good' },
  { value: 'USED_ACCEPTABLE', label: 'Used - Acceptable' },
  { value: 'FOR_PARTS_OR_NOT_WORKING', label: 'For Parts or Not Working' },
]

async function fetchEbayPolicies() {
  if (ebayFulfillmentPolicies.value.length > 0) return // already fetched
  ebayPoliciesLoading.value = true
  ebayPoliciesError.value = ''
  try {
    const [fulfillment, payment, ret] = await ebayGetBusinessPolicies()
    ebayFulfillmentPolicies.value = fulfillment
    ebayPaymentPolicies.value = payment
    ebayReturnPolicies.value = ret
  } catch (e: any) {
    console.error('Failed to fetch eBay policies:', e)
    const msg = e?.toString() || ''
    if (msg.includes('not eligible for Business Policy')) {
      ebayPoliciesError.value = 'Your eBay account is not eligible for Business Policies. You can still publish without them.'
    } else {
      ebayPoliciesError.value = msg || 'Failed to load policies'
    }
  } finally {
    ebayPoliciesLoading.value = false
  }
}

let ebayCategoryDebounce: ReturnType<typeof setTimeout> | null = null

function onEbayCategoryInput() {
  if (ebayCategoryDebounce) clearTimeout(ebayCategoryDebounce)
  const q = ebayCategoryQuery.value.trim()
  if (q.length < 2) {
    ebayCategorySuggestions.value = []
    return
  }
  ebayCategoryDebounce = setTimeout(async () => {
    ebayCategoryLoading.value = true
    try {
      ebayCategorySuggestions.value = await ebayGetCategorySuggestions(q)
    } catch (e) {
      console.error('Category suggestions failed:', e)
      ebayCategorySuggestions.value = []
    } finally {
      ebayCategoryLoading.value = false
    }
  }, 300)
}

async function selectEbayCategory(suggestion: EbayCategorySuggestion) {
  ebayCategoryId.value = suggestion.category.category_id
  ebayCategoryName.value = suggestion.category.category_name
  ebayCategorySuggestions.value = []
  ebayCategoryQuery.value = ''

  // Auto-fetch aspects for the selected category
  ebayAspectsLoading.value = true
  try {
    ebayAspectMetadata.value = await ebayGetCategoryAspects(suggestion.category.category_id)
    // Initialize empty arrays for required aspects
    for (const aspect of ebayAspectMetadata.value) {
      if (!ebayAspects.value[aspect.name]) {
        ebayAspects.value[aspect.name] = []
      }
    }
  } catch (e) {
    console.error('Failed to fetch aspects:', e)
    ebayAspectMetadata.value = []
  } finally {
    ebayAspectsLoading.value = false
  }
}

function clearEbayCategory() {
  ebayCategoryId.value = ''
  ebayCategoryName.value = ''
  ebayAspectMetadata.value = []
  ebayAspects.value = {}
}

function getCategoryBreadcrumb(suggestion: EbayCategorySuggestion): string {
  const ancestors = suggestion.ancestors || []
  const parts = [...ancestors.map(a => a.category_name).reverse(), suggestion.category.category_name]
  return parts.join(' > ')
}

function getAspectValue(name: string): string {
  return (ebayAspects.value[name] || []).join(', ')
}

function setAspectValue(name: string, value: string) {
  if (value.trim()) {
    ebayAspects.value[name] = value.split(',').map(v => v.trim()).filter(Boolean)
  } else {
    ebayAspects.value[name] = []
  }
}

function setAspectSingleValue(name: string, value: string) {
  ebayAspects.value[name] = value ? [value] : []
}

// Loading state
const loading = ref(true)
const loadError = ref('')

// Source data
const product = ref<Awaited<ReturnType<typeof getProduct>> | null>(null)
const mappings = ref<PlatformMapping[]>([])
const sourceListing = ref<FullListing | null>(null)
const vendorListings = ref<VendorListingForProduct[]>([])
const comparisonListings = ref<FullListing[]>([])

// Variant state
const variants = ref<ProductVariant[]>([])
const selectedVariantIds = ref<string[]>([])
const initialLoadComplete = ref(false)

// Platforms that support multi-variant listings
const platformSupportsMultiVariant = computed(() => {
  const plat = selectedPlatform.value
  return plat === 'ebay' || plat === 'squarespace'
})

const showVariantSelector = computed(() => variants.value.length > 1)

const variantSelectOptions = computed(() =>
  variants.value.map(v => {
    const attrs = Object.values(v.attributes).join(' / ')
    const label = attrs ? `${attrs} — ${v.sku}` : (v.name ? `${v.name} — ${v.sku}` : v.sku)
    return { label, value: v.id }
  })
)

// Single-variant select model (for platforms without multi-variant support)
const singleVariantModel = computed({
  get: () => selectedVariantIds.value[0] ?? '',
  set: (val: string) => { selectedVariantIds.value = val ? [val] : [] },
})

// Primary selected variant (first in list, or the only one)
const selectedVariant = computed<ProductVariant | null>(() => {
  if (variants.value.length <= 1) return variants.value[0] ?? null
  if (selectedVariantIds.value.length === 0) return null
  return variants.value.find(v => v.id === selectedVariantIds.value[0]) ?? null
})

const selectedVariants = computed(() =>
  variants.value.filter(v => selectedVariantIds.value.includes(v.id))
)

const variantSelectionComplete = computed(() => {
  if (variants.value.length <= 1) return true
  return selectedVariantIds.value.length > 0
})

// --- Variant builder (attribute-based selection for multi-variant platforms) ---
const builderSelections = ref<Record<string, string>>({})

const variantAttributeKeys = computed(() => {
  const keys = new Set<string>()
  for (const v of variants.value) {
    for (const k of Object.keys(v.attributes)) keys.add(k)
  }
  return [...keys]
})

// Only consider variants that haven't been added yet
const availableVariants = computed(() =>
  variants.value.filter(v => !selectedVariantIds.value.includes(v.id))
)

const variantAttributeOptions = computed(() => {
  const options: Record<string, string[]> = {}
  for (const key of variantAttributeKeys.value) {
    const vals = new Set<string>()
    for (const v of availableVariants.value) {
      const matches = Object.entries(builderSelections.value).every(
        ([k, sel]) => k === key || !sel || v.attributes[k] === sel
      )
      if (matches && v.attributes[key]) vals.add(v.attributes[key])
    }
    options[key] = [...vals].sort()
  }
  return options
})

const builderMatch = computed<ProductVariant | null>(() => {
  if (variantAttributeKeys.value.length === 0) return null
  const sels = builderSelections.value
  const allSelected = variantAttributeKeys.value.every(k => sels[k])
  if (!allSelected) return null
  return availableVariants.value.find(v =>
    variantAttributeKeys.value.every(k => v.attributes[k] === sels[k])
  ) ?? null
})

function onBuilderSelectionChange(key: string, value: string) {
  builderSelections.value = { ...builderSelections.value, [key]: value }
}

function addSelectedVariant() {
  const match = builderMatch.value
  if (match && !selectedVariantIds.value.includes(match.id)) {
    selectedVariantIds.value = [...selectedVariantIds.value, match.id]
  }
  // Reset builder
  builderSelections.value = {}
}

function addAllVariants() {
  selectedVariantIds.value = variants.value.map(v => v.id)
  builderSelections.value = {}
}

function removeVariant(id: string) {
  selectedVariantIds.value = selectedVariantIds.value.filter(v => v !== id)
}

function variantLabel(v: ProductVariant): string {
  const attrs = Object.values(v.attributes).join(' / ')
  return attrs ? `${attrs} — ${v.sku}` : (v.name !== 'Default' ? `${v.name} — ${v.sku}` : v.sku)
}

// Available sources for the selected variant (platform listings + vendor listings)
const variantSources = computed(() => {
  const sources: { label: string; type: 'platform' | 'vendor'; listing?: FullListing; vendorListing?: VendorListingForProduct }[] = []
  const vid = selectedVariant.value?.id ?? null
  // Platform listings mapped to this variant
  for (const l of comparisonListings.value) {
    if (l.variant_id === vid) {
      const platLabel = { ebay: 'eBay', squarespace: 'Squarespace', xmrbazaar: 'XMR Bazaar', amazon: 'Amazon' }[l.platform] || l.platform
      sources.push({ label: `${platLabel}: ${l.title}`, type: 'platform', listing: l })
    }
  }
  // Vendor listings for this variant
  for (const vl of vendorListings.value) {
    if (vl.variant_id === vid) {
      sources.push({ label: `${vl.plugin_name}: ${vl.title}`, type: 'vendor', vendorListing: vl })
    }
  }
  return sources
})

function applySource(source: typeof variantSources.value[0]) {
  if (source.type === 'platform' && source.listing) {
    const l = source.listing
    formTitle.value = l.title
    formDescription.value = l.description?.html || l.description?.plain_text || ''
    formPhotos.value = l.photos?.map(p => p.url) || []
    if (l.price) {
      formPriceAmount.value = l.price.amount
      formPriceCurrency.value = l.price.currency
    }
    sourceListing.value = l
  } else if (source.type === 'vendor' && source.vendorListing) {
    const vl = source.vendorListing
    formTitle.value = product.value?.name || vl.title || ''
    const fallbackDesc = vendorListings.value.find(v => v.extras?.description)?.extras?.description || ''
    formDescription.value = vl.extras?.description || fallbackDesc
    if (vl.price != null) {
      formPriceAmount.value = vl.price
      formPriceCurrency.value = vl.currency || 'USD'
    }
    // Collect images from all vendor listings for this variant
    const images = new Set<string>()
    for (const v of vendorListings.value) {
      if (v.variant_id === selectedVariant.value?.id && v.image_url) images.add(v.image_url)
    }
    formPhotos.value = [...images]
  }
}

// Editable form fields
const formTitle = ref('')
const formDescription = ref('')
const formPriceAmount = ref<number | null>(null)
const formPriceCurrency = ref('USD')

// Photos (populated from source listing on mount)
const formPhotos = ref<string[]>([])
const newPhotoUrl = ref('')

function removePhoto(index: number) {
  formPhotos.value.splice(index, 1)
}

function addPhoto() {
  const url = newPhotoUrl.value.trim()
  if (url && !formPhotos.value.includes(url)) {
    formPhotos.value.push(url)
    newPhotoUrl.value = ''
  }
}

// Edit mode detection
const isEditMode = computed(() => route.query.mode === 'edit')
const editPlatform = computed(() => route.query.platform as Platform | undefined)
const editItemId = computed(() => route.query.itemId as string | undefined)

// Target selection
const selectedPlatform = ref<Platform | null>(null)

// Auto-fetch eBay policies when platform switches to eBay; reset variant selection on platform change
watch(selectedPlatform, (plat) => {
  if (plat === 'ebay') fetchEbayPolicies()
  // Reset variant selection when switching platforms (multi/single mode may differ)
  if (variants.value.length > 1) selectedVariantIds.value = []
})

// Publish state
const publishing = ref(false)
const publishError = ref('')

// --- XMR Bazaar-specific fields ---
// Category values are lowercase slugs as used in the XMR Bazaar form
const xmrCategories = [
  { value: 'art', label: 'Art' },
  { value: 'automobile', label: 'Automobile' },
  { value: 'books', label: 'Books' },
  { value: 'clothes', label: 'Clothes' },
  { value: 'collectables', label: 'Collectables' },
  { value: 'electronics', label: 'Electronics' },
  { value: 'food', label: 'Food' },
  { value: 'furniture', label: 'Furniture' },
  { value: 'games', label: 'Games' },
  { value: 'gift_cards', label: 'Gift cards' },
  { value: 'health', label: 'Health' },
  { value: 'gold_silver', label: 'Gold/Silver' },
  { value: 'music', label: 'Music' },
  { value: 'nsfw', label: 'NSFW' },
  { value: 'other', label: 'Other' },
  { value: 'pets', label: 'Pets' },
  { value: 'real_estate', label: 'Real Estate' },
  { value: 'sports', label: 'Sports' },
  { value: 'tools', label: 'Tools' },
]

const xmrCountries = [
  { value: 'AF', label: 'Afghanistan' },
  { value: 'AX', label: 'Åland Islands' },
  { value: 'AL', label: 'Albania' },
  { value: 'DZ', label: 'Algeria' },
  { value: 'AS', label: 'American Samoa' },
  { value: 'AD', label: 'Andorra' },
  { value: 'AO', label: 'Angola' },
  { value: 'AI', label: 'Anguilla' },
  { value: 'AQ', label: 'Antarctica' },
  { value: 'AG', label: 'Antigua and Barbuda' },
  { value: 'AR', label: 'Argentina' },
  { value: 'AM', label: 'Armenia' },
  { value: 'AW', label: 'Aruba' },
  { value: 'AU', label: 'Australia' },
  { value: 'AT', label: 'Austria' },
  { value: 'AZ', label: 'Azerbaijan' },
  { value: 'BS', label: 'Bahamas' },
  { value: 'BH', label: 'Bahrain' },
  { value: 'BD', label: 'Bangladesh' },
  { value: 'BB', label: 'Barbados' },
  { value: 'BY', label: 'Belarus' },
  { value: 'BE', label: 'Belgium' },
  { value: 'BZ', label: 'Belize' },
  { value: 'BJ', label: 'Benin' },
  { value: 'BM', label: 'Bermuda' },
  { value: 'BT', label: 'Bhutan' },
  { value: 'BO', label: 'Bolivia' },
  { value: 'BQ', label: 'Bonaire, Sint Eustatius and Saba' },
  { value: 'BA', label: 'Bosnia and Herzegovina' },
  { value: 'BW', label: 'Botswana' },
  { value: 'BV', label: 'Bouvet Island' },
  { value: 'BR', label: 'Brazil' },
  { value: 'IO', label: 'British Indian Ocean Territory' },
  { value: 'BN', label: 'Brunei Darussalam' },
  { value: 'BG', label: 'Bulgaria' },
  { value: 'BF', label: 'Burkina Faso' },
  { value: 'BI', label: 'Burundi' },
  { value: 'KH', label: 'Cambodia' },
  { value: 'CM', label: 'Cameroon' },
  { value: 'CA', label: 'Canada' },
  { value: 'CV', label: 'Cape Verde' },
  { value: 'KY', label: 'Cayman Islands' },
  { value: 'CF', label: 'Central African Republic' },
  { value: 'TD', label: 'Chad' },
  { value: 'CL', label: 'Chile' },
  { value: 'CN', label: 'China' },
  { value: 'CX', label: 'Christmas Island' },
  { value: 'CC', label: 'Cocos (Keeling) Islands' },
  { value: 'CO', label: 'Colombia' },
  { value: 'KM', label: 'Comoros' },
  { value: 'CG', label: 'Congo' },
  { value: 'CD', label: 'Congo, Democratic Republic' },
  { value: 'CK', label: 'Cook Islands' },
  { value: 'CR', label: 'Costa Rica' },
  { value: 'CI', label: "Côte d'Ivoire" },
  { value: 'HR', label: 'Croatia' },
  { value: 'CU', label: 'Cuba' },
  { value: 'CW', label: 'Curaçao' },
  { value: 'CY', label: 'Cyprus' },
  { value: 'CZ', label: 'Czech Republic' },
  { value: 'DK', label: 'Denmark' },
  { value: 'DJ', label: 'Djibouti' },
  { value: 'DM', label: 'Dominica' },
  { value: 'DO', label: 'Dominican Republic' },
  { value: 'EC', label: 'Ecuador' },
  { value: 'EG', label: 'Egypt' },
  { value: 'SV', label: 'El Salvador' },
  { value: 'GQ', label: 'Equatorial Guinea' },
  { value: 'ER', label: 'Eritrea' },
  { value: 'EE', label: 'Estonia' },
  { value: 'ET', label: 'Ethiopia' },
  { value: 'FK', label: 'Falkland Islands' },
  { value: 'FO', label: 'Faroe Islands' },
  { value: 'FJ', label: 'Fiji' },
  { value: 'FI', label: 'Finland' },
  { value: 'FR', label: 'France' },
  { value: 'GF', label: 'French Guiana' },
  { value: 'PF', label: 'French Polynesia' },
  { value: 'TF', label: 'French Southern Territories' },
  { value: 'GA', label: 'Gabon' },
  { value: 'GM', label: 'Gambia' },
  { value: 'GE', label: 'Georgia' },
  { value: 'DE', label: 'Germany' },
  { value: 'GH', label: 'Ghana' },
  { value: 'GI', label: 'Gibraltar' },
  { value: 'GR', label: 'Greece' },
  { value: 'GL', label: 'Greenland' },
  { value: 'GD', label: 'Grenada' },
  { value: 'GP', label: 'Guadeloupe' },
  { value: 'GU', label: 'Guam' },
  { value: 'GT', label: 'Guatemala' },
  { value: 'GG', label: 'Guernsey' },
  { value: 'GN', label: 'Guinea' },
  { value: 'GW', label: 'Guinea-Bissau' },
  { value: 'GY', label: 'Guyana' },
  { value: 'HT', label: 'Haiti' },
  { value: 'HM', label: 'Heard Island and McDonald Islands' },
  { value: 'VA', label: 'Holy See (Vatican City State)' },
  { value: 'HN', label: 'Honduras' },
  { value: 'HK', label: 'Hong Kong' },
  { value: 'HU', label: 'Hungary' },
  { value: 'IS', label: 'Iceland' },
  { value: 'IN', label: 'India' },
  { value: 'ID', label: 'Indonesia' },
  { value: 'IR', label: 'Iran' },
  { value: 'IQ', label: 'Iraq' },
  { value: 'IE', label: 'Ireland' },
  { value: 'IM', label: 'Isle of Man' },
  { value: 'IL', label: 'Israel' },
  { value: 'IT', label: 'Italy' },
  { value: 'JM', label: 'Jamaica' },
  { value: 'JP', label: 'Japan' },
  { value: 'JE', label: 'Jersey' },
  { value: 'JO', label: 'Jordan' },
  { value: 'KZ', label: 'Kazakhstan' },
  { value: 'KE', label: 'Kenya' },
  { value: 'KI', label: 'Kiribati' },
  { value: 'KP', label: "Korea, Democratic People's Republic" },
  { value: 'KR', label: 'Korea, Republic of' },
  { value: 'KW', label: 'Kuwait' },
  { value: 'KG', label: 'Kyrgyzstan' },
  { value: 'LA', label: "Lao People's Democratic Republic" },
  { value: 'LV', label: 'Latvia' },
  { value: 'LB', label: 'Lebanon' },
  { value: 'LS', label: 'Lesotho' },
  { value: 'LR', label: 'Liberia' },
  { value: 'LY', label: 'Libya' },
  { value: 'LI', label: 'Liechtenstein' },
  { value: 'LT', label: 'Lithuania' },
  { value: 'LU', label: 'Luxembourg' },
  { value: 'MO', label: 'Macao' },
  { value: 'MK', label: 'Macedonia' },
  { value: 'MG', label: 'Madagascar' },
  { value: 'MW', label: 'Malawi' },
  { value: 'MY', label: 'Malaysia' },
  { value: 'MV', label: 'Maldives' },
  { value: 'ML', label: 'Mali' },
  { value: 'MT', label: 'Malta' },
  { value: 'MH', label: 'Marshall Islands' },
  { value: 'MQ', label: 'Martinique' },
  { value: 'MR', label: 'Mauritania' },
  { value: 'MU', label: 'Mauritius' },
  { value: 'YT', label: 'Mayotte' },
  { value: 'MX', label: 'Mexico' },
  { value: 'FM', label: 'Micronesia' },
  { value: 'MD', label: 'Moldova' },
  { value: 'MC', label: 'Monaco' },
  { value: 'MN', label: 'Mongolia' },
  { value: 'ME', label: 'Montenegro' },
  { value: 'MS', label: 'Montserrat' },
  { value: 'MA', label: 'Morocco' },
  { value: 'MZ', label: 'Mozambique' },
  { value: 'MM', label: 'Myanmar' },
  { value: 'NA', label: 'Namibia' },
  { value: 'NR', label: 'Nauru' },
  { value: 'NP', label: 'Nepal' },
  { value: 'NL', label: 'Netherlands' },
  { value: 'NC', label: 'New Caledonia' },
  { value: 'NZ', label: 'New Zealand' },
  { value: 'NI', label: 'Nicaragua' },
  { value: 'NE', label: 'Niger' },
  { value: 'NG', label: 'Nigeria' },
  { value: 'NU', label: 'Niue' },
  { value: 'NF', label: 'Norfolk Island' },
  { value: 'MP', label: 'Northern Mariana Islands' },
  { value: 'NO', label: 'Norway' },
  { value: 'OM', label: 'Oman' },
  { value: 'PK', label: 'Pakistan' },
  { value: 'PW', label: 'Palau' },
  { value: 'PS', label: 'Palestinian Territory' },
  { value: 'PA', label: 'Panama' },
  { value: 'PG', label: 'Papua New Guinea' },
  { value: 'PY', label: 'Paraguay' },
  { value: 'PE', label: 'Peru' },
  { value: 'PH', label: 'Philippines' },
  { value: 'PN', label: 'Pitcairn' },
  { value: 'PL', label: 'Poland' },
  { value: 'PT', label: 'Portugal' },
  { value: 'PR', label: 'Puerto Rico' },
  { value: 'QA', label: 'Qatar' },
  { value: 'RE', label: 'Réunion' },
  { value: 'RO', label: 'Romania' },
  { value: 'RU', label: 'Russian Federation' },
  { value: 'RW', label: 'Rwanda' },
  { value: 'BL', label: 'Saint Barthélemy' },
  { value: 'SH', label: 'Saint Helena' },
  { value: 'KN', label: 'Saint Kitts and Nevis' },
  { value: 'LC', label: 'Saint Lucia' },
  { value: 'MF', label: 'Saint Martin (French part)' },
  { value: 'PM', label: 'Saint Pierre and Miquelon' },
  { value: 'VC', label: 'Saint Vincent and the Grenadines' },
  { value: 'WS', label: 'Samoa' },
  { value: 'SM', label: 'San Marino' },
  { value: 'ST', label: 'Sao Tome and Principe' },
  { value: 'SA', label: 'Saudi Arabia' },
  { value: 'SN', label: 'Senegal' },
  { value: 'RS', label: 'Serbia' },
  { value: 'SC', label: 'Seychelles' },
  { value: 'SL', label: 'Sierra Leone' },
  { value: 'SG', label: 'Singapore' },
  { value: 'SX', label: 'Sint Maarten (Dutch part)' },
  { value: 'SK', label: 'Slovakia' },
  { value: 'SI', label: 'Slovenia' },
  { value: 'SB', label: 'Solomon Islands' },
  { value: 'SO', label: 'Somalia' },
  { value: 'ZA', label: 'South Africa' },
  { value: 'GS', label: 'South Georgia and the South Sandwich Islands' },
  { value: 'SS', label: 'South Sudan' },
  { value: 'ES', label: 'Spain' },
  { value: 'LK', label: 'Sri Lanka' },
  { value: 'SD', label: 'Sudan' },
  { value: 'SR', label: 'Suriname' },
  { value: 'SJ', label: 'Svalbard and Jan Mayen' },
  { value: 'SZ', label: 'Swaziland' },
  { value: 'SE', label: 'Sweden' },
  { value: 'CH', label: 'Switzerland' },
  { value: 'SY', label: 'Syrian Arab Republic' },
  { value: 'TW', label: 'Taiwan' },
  { value: 'TJ', label: 'Tajikistan' },
  { value: 'TZ', label: 'Tanzania' },
  { value: 'TH', label: 'Thailand' },
  { value: 'TL', label: 'Timor-Leste' },
  { value: 'TG', label: 'Togo' },
  { value: 'TK', label: 'Tokelau' },
  { value: 'TO', label: 'Tonga' },
  { value: 'TT', label: 'Trinidad and Tobago' },
  { value: 'TN', label: 'Tunisia' },
  { value: 'TR', label: 'Turkey' },
  { value: 'TM', label: 'Turkmenistan' },
  { value: 'TC', label: 'Turks and Caicos Islands' },
  { value: 'TV', label: 'Tuvalu' },
  { value: 'UG', label: 'Uganda' },
  { value: 'UA', label: 'Ukraine' },
  { value: 'AE', label: 'United Arab Emirates' },
  { value: 'GB', label: 'United Kingdom' },
  { value: 'US', label: 'United States' },
  { value: 'UM', label: 'United States Minor Outlying Islands' },
  { value: 'UY', label: 'Uruguay' },
  { value: 'UZ', label: 'Uzbekistan' },
  { value: 'VU', label: 'Vanuatu' },
  { value: 'VE', label: 'Venezuela' },
  { value: 'VN', label: 'Viet Nam' },
  { value: 'VG', label: 'Virgin Islands, British' },
  { value: 'VI', label: 'Virgin Islands, U.S.' },
  { value: 'WF', label: 'Wallis and Futuna' },
  { value: 'EH', label: 'Western Sahara' },
  { value: 'YE', label: 'Yemen' },
  { value: 'ZM', label: 'Zambia' },
  { value: 'ZW', label: 'Zimbabwe' },
]

const xmrCategory = ref('other')
const xmrTags = ref('')
const xmrDelivery = ref('digital')
const xmrCity = ref('')
const xmrCountry = ref('US')
const xmrFlexiblePrice = ref(false)
const xmrInPersonPickup = ref(false)
const xmrPickupShippingCost = ref('')
const xmrDomesticShipping = ref(true)
const xmrDomesticShippingCost = ref('40')
const xmrInternationalShipping = ref(false)
const xmrInternationalShippingCost = ref('150')
const xmrPaymentMethod = ref('direct')
const xmrMoneroAddress = ref('')
const xmrStock = ref('one-time')
const xmrPrivateListing = ref(false)
const xmrTerms = ref(true)

const xmrCurrencies = [
  { value: 'USD', label: 'USD ($)' },
  { value: 'EUR', label: 'EUR (€)' },
  { value: 'CAD', label: 'CAD ($)' },
  { value: 'GBP', label: 'GBP (£)' },
  { value: 'AUD', label: 'AUD ($)' },
  { value: 'AED', label: 'AED (د.إ.)' },
  { value: 'ARS', label: 'ARS ($)' },
  { value: 'BDT', label: 'BDT (৳)' },
  { value: 'BHD', label: 'BHD (د.ب.)' },
  { value: 'BMD', label: 'BMD ($)' },
  { value: 'BRL', label: 'BRL (R$)' },
  { value: 'CHF', label: 'CHF (CHF)' },
  { value: 'CLP', label: 'CLP ($)' },
  { value: 'CNY', label: 'CNY (¥)' },
  { value: 'CZK', label: 'CZK (Kč)' },
  { value: 'DKK', label: 'DKK (kr.)' },
  { value: 'HKD', label: 'HKD (HK$)' },
  { value: 'HUF', label: 'HUF (Ft)' },
  { value: 'IDR', label: 'IDR (Rp)' },
  { value: 'ILS', label: 'ILS (₪)' },
  { value: 'INR', label: 'INR (₹)' },
  { value: 'JPY', label: 'JPY (¥)' },
  { value: 'KRW', label: 'KRW (₩)' },
  { value: 'KWD', label: 'KWD (د.ك.)' },
  { value: 'LKR', label: 'LKR (₨)' },
  { value: 'MMK', label: 'MMK (K)' },
  { value: 'MXN', label: 'MXN ($)' },
  { value: 'MYR', label: 'MYR (RM)' },
  { value: 'NGN', label: 'NGN (₦)' },
  { value: 'NOK', label: 'NOK (kr)' },
  { value: 'NZD', label: 'NZD ($)' },
  { value: 'PHP', label: 'PHP (₱)' },
  { value: 'PKR', label: 'PKR (₨)' },
  { value: 'PLN', label: 'PLN (zł)' },
  { value: 'RUB', label: 'RUB (₽)' },
  { value: 'SAR', label: 'SAR (﷼)' },
  { value: 'SEK', label: 'SEK (kr)' },
  { value: 'SGD', label: 'SGD ($)' },
  { value: 'THB', label: 'THB (฿)' },
  { value: 'TRY', label: 'TRY (₺)' },
  { value: 'TWD', label: 'TWD (NT$)' },
  { value: 'UAH', label: 'UAH (₴)' },
  { value: 'VEF', label: 'VEF (Bs. F.)' },
  { value: 'VND', label: 'VND (₫)' },
  { value: 'ZAR', label: 'ZAR (R)' },
  { value: 'XAG', label: 'XAG (Silver Ounce)' },
  { value: 'XAU', label: 'XAU (Gold Ounce)' },
  { value: 'XMR', label: 'XMR (Monero)' },
  { value: 'FIRO', label: 'FIRO (Firo)' },
]

const isXmr = computed(() => selectedPlatform.value === 'xmrbazaar')

const publishablePlatforms = computed(() => {
  if (isEditMode.value && editPlatform.value) {
    return [editPlatform.value]
  }
  // Check per-variant: allow platform if selected variant doesn't already have a mapping on it
  const vid = selectedVariant.value?.id ?? ''
  const mappedPlatformVariants = new Set(
    mappings.value.map(m => `${m.platform}:${m.variant_id ?? ''}`)
  )
  return (Object.entries(platformCapabilities.value) as [Platform, PlatformCapabilities][])
    .filter(([plat, caps]) => caps.can_create_listing && !mappedPlatformVariants.has(`${plat}:${vid}`))
    .map(([plat]) => plat)
})

// ── Platform tab items ──
const platformTabItems = computed(() =>
  publishablePlatforms.value.map(plat => ({ label: platformLabels[plat], value: plat }))
)

const selectedPlatformModel = computed({
  get: () => selectedPlatform.value ?? '',
  set: (val: string) => { selectedPlatform.value = val as Platform },
})

// Validation for XMR Bazaar
const xmrValid = computed(() => {
  if (!isXmr.value) return true
  if (!xmrMoneroAddress.value.trim()) return false
  if (!xmrTerms.value) return false
  return true
})

// Validation for eBay
const ebayValid = computed(() => {
  if (!isEbay.value) return true
  // Category is required for new listings
  if (!isEditMode.value && !ebayCategoryId.value) return false
  // Check required aspects
  for (const aspect of ebayAspectMetadata.value) {
    if (aspect.constraint?.required && (!ebayAspects.value[aspect.name] || ebayAspects.value[aspect.name].length === 0)) {
      return false
    }
  }
  return true
})

const canPublish = computed(() => {
  return selectedPlatform.value && formTitle.value.trim() && xmrValid.value && ebayValid.value
})

/** Strip HTML tags client-side, preserving block structure and list bullets */
function stripHtml(html: string): string {
  const doc = new DOMParser().parseFromString(html, 'text/html')
  const blockTags = new Set([
    'P', 'DIV', 'BR', 'HR', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6',
    'UL', 'OL', 'BLOCKQUOTE', 'SECTION', 'ARTICLE', 'TR', 'HEADER', 'FOOTER',
  ])
  function walk(node: Node): string {
    if (node.nodeType === Node.TEXT_NODE) return node.textContent || ''
    if (node.nodeType !== Node.ELEMENT_NODE) return ''
    const tag = (node as Element).tagName
    const isBlock = blockTags.has(tag)
    const isLi = tag === 'LI'
    let result = ''
    if (isBlock || isLi) result += '\n'
    if (isLi) result += '· '
    for (const child of Array.from(node.childNodes)) {
      result += walk(child)
    }
    if (isBlock && !result.endsWith('\n')) result += '\n'
    return result
  }
  return walk(doc.body).replace(/\n{3,}/g, '\n\n').trim()
}

const { companiesInitialized } = useCompanyContext()

async function initPublish() {
  if (!companiesInitialized.value) return
  try {
    const [prod, maps, vd, variantData] = await Promise.all([
      getProduct(productId),
      getMappings(productId),
      invoke<VendorListingForProduct[]>('get_vendor_listings_for_product', { productId }).catch(() => [] as VendorListingForProduct[]),
      listVariants(productId).catch(() => [] as ProductVariant[]),
    ])
    product.value = prod
    mappings.value = maps
    vendorListings.value = vd
    variants.value = variantData

    // Fetch capabilities + config in parallel
    fetchCapabilities()
    fetchConfig().then(() => {
      if (config.value?.xmrbazaar?.monero_address) {
        xmrMoneroAddress.value = config.value.xmrbazaar.monero_address
      }
    })

    if (maps.length === 0 && vd.length === 0 && !isEditMode.value) {
      // No source data at all — still allow publishing with just product name
      formTitle.value = prod?.name || ''
    }

    // Fetch full listing data from platform mappings (if any)
    const listings = maps.length > 0 ? await diffListings(productId) : []
    comparisonListings.value = listings

    if (isEditMode.value && editPlatform.value && editItemId.value) {
      // Edit mode: find the specific listing to edit and pre-populate
      selectedPlatform.value = editPlatform.value
      const editListing = listings.find(
        l => l.platform === editPlatform.value && l.platform_item_id === editItemId.value
      )
      if (editListing) {
        sourceListing.value = editListing
        formTitle.value = editListing.title
        formDescription.value = editListing.description?.html
          || editListing.description?.plain_text
          || ''
        formPhotos.value = editListing.photos?.map(p => p.url) || []
        if (editListing.price) {
          formPriceAmount.value = editListing.price.amount
          formPriceCurrency.value = editListing.price.currency
        }
        // Pre-populate eBay extras
        if (editListing.extras && editPlatform.value === 'ebay') {
          const ex = editListing.extras
          if (ex.ebay_category_id) {
            ebayCategoryId.value = ex.ebay_category_id
            ebayCategoryName.value = ex.ebay_category_name || ex.ebay_category_id
            ebayGetCategoryAspects(ex.ebay_category_id).then(aspects => {
              ebayAspectMetadata.value = aspects
            }).catch(() => {})
          }
          if (ex.ebay_condition) ebayCondition.value = ex.ebay_condition
          if (ex.ebay_condition_description) ebayConditionDescription.value = ex.ebay_condition_description
          if (ex.ebay_format) ebayFormat.value = ex.ebay_format
          if (ex.ebay_fulfillment_policy_id) ebayFulfillmentPolicyId.value = ex.ebay_fulfillment_policy_id
          if (ex.ebay_payment_policy_id) ebayPaymentPolicyId.value = ex.ebay_payment_policy_id
          if (ex.ebay_return_policy_id) ebayReturnPolicyId.value = ex.ebay_return_policy_id
          if (ex.ebay_aspects) {
            try {
              ebayAspects.value = JSON.parse(ex.ebay_aspects)
            } catch {}
          }
        }
      } else {
        loadError.value = 'Could not find the listing to edit.'
      }
    } else if (variantData.length <= 1) {
      // Single variant — auto-prefill from best source
      const firstListing = listings[0]
      const firstVendor = vd[0]
      if (firstListing) {
        sourceListing.value = firstListing
        formTitle.value = firstListing.title
        formDescription.value = firstListing.description?.html || firstListing.description?.plain_text || ''
        formPhotos.value = firstListing.photos?.map(p => p.url) || []
        if (firstListing.price) {
          formPriceAmount.value = firstListing.price.amount
          formPriceCurrency.value = firstListing.price.currency
        }
      } else if (firstVendor) {
        formTitle.value = prod?.name || firstVendor.title || ''
        const fallbackDesc = vd.find(v => v.extras?.description)?.extras?.description || ''
        formDescription.value = firstVendor.extras?.description || fallbackDesc
        if (firstVendor.price != null) {
          formPriceAmount.value = firstVendor.price
          formPriceCurrency.value = firstVendor.currency || 'USD'
        }
        const images = new Set<string>()
        for (const v of vd) {
          if (v.image_url) images.add(v.image_url)
        }
        formPhotos.value = [...images]
      } else {
        formTitle.value = prod?.name || ''
      }
    } else {
      // Multi-variant — don't auto-prefill, let user select variant first
      formTitle.value = prod?.name || ''
    }
  } catch (e: any) {
    loadError.value = e?.toString() || 'Failed to load listing data'
  } finally {
    loading.value = false
    initialLoadComplete.value = true
  }
}

onMounted(initPublish)
watch(companiesInitialized, initPublish)

// When user changes variant, auto-prefill from best available source
watch(selectedVariant, (variant) => {
  if (!variant || !initialLoadComplete.value || isEditMode.value) return
  const sources = variantSources.value
  if (sources.length > 0) {
    // Auto-apply first available source
    applySource(sources[0])
  } else {
    // No mapped sources — use variant's own data
    formTitle.value = product.value?.name || ''
    if (variant.image_url) {
      formPhotos.value = [variant.image_url]
    }
  }
})

async function handlePublish() {
  if (!selectedPlatform.value || !formTitle.value.trim()) return
  if (!isEditMode.value && !xmrValid.value) return
  if (!ebayValid.value) return

  publishing.value = true
  publishError.value = ''

  try {
    // Build eBay extras
    let ebayExtras: Record<string, string> | undefined
    if (isEbay.value) {
      ebayExtras = {}
      if (ebayCategoryId.value) ebayExtras.ebay_category_id = ebayCategoryId.value
      if (ebayCategoryName.value) ebayExtras.ebay_category_name = ebayCategoryName.value
      ebayExtras.ebay_condition = ebayCondition.value
      if (ebayConditionDescription.value.trim()) ebayExtras.ebay_condition_description = ebayConditionDescription.value.trim()
      if (Object.keys(ebayAspects.value).length > 0) {
        // Filter out empty arrays
        const filtered: Record<string, string[]> = {}
        for (const [k, v] of Object.entries(ebayAspects.value)) {
          if (v.length > 0) filtered[k] = v
        }
        if (Object.keys(filtered).length > 0) {
          ebayExtras.ebay_aspects = JSON.stringify(filtered)
        }
      }
      ebayExtras.ebay_format = ebayFormat.value
      if (ebayFulfillmentPolicyId.value.trim()) ebayExtras.ebay_fulfillment_policy_id = ebayFulfillmentPolicyId.value.trim()
      if (ebayPaymentPolicyId.value.trim()) ebayExtras.ebay_payment_policy_id = ebayPaymentPolicyId.value.trim()
      if (ebayReturnPolicyId.value.trim()) ebayExtras.ebay_return_policy_id = ebayReturnPolicyId.value.trim()
    }

    if (isEditMode.value && editItemId.value) {
      // Update existing listing
      let description = formDescription.value.trim() || null
      if (isXmr.value && description) {
        description = stripHtml(description)
      }

      await updateListingOnPlatform({
        platform: selectedPlatform.value,
        platformItemId: editItemId.value,
        title: formTitle.value.trim(),
        descriptionHtml: description,
        priceAmount: formPriceAmount.value,
        priceCurrency: formPriceAmount.value !== null ? formPriceCurrency.value : null,
        extras: ebayExtras,
      })
    } else {
      // Build extras for XMR Bazaar — field names must match the actual form fields
      let extras: Record<string, string> | undefined
      if (isXmr.value) {
        extras = {
          category: xmrCategory.value,
          tags: xmrTags.value,
          currency: formPriceCurrency.value,
          stock: xmrStock.value,
          // location_type: "online" for digital, "local" for physical
          location_type: xmrDelivery.value === 'digital' ? 'online' : 'local',
          price_flexibility: xmrFlexiblePrice.value ? 'flexible' : '',
          in_person_pickup: xmrInPersonPickup.value ? 'enabled' : '',
          in_person_pickup_price: xmrPickupShippingCost.value,
          domestic_shipping: xmrDomesticShipping.value ? 'enabled' : '',
          domestic_shipping_price: xmrDomesticShippingCost.value,
          international_shipping: xmrInternationalShipping.value ? 'enabled' : '',
          international_shipping_price: xmrInternationalShippingCost.value,
          payment_method_direct: xmrPaymentMethod.value === 'direct' ? 'enabled' : '',
          payment_method_escrow: xmrPaymentMethod.value === 'escrow' ? 'enabled' : '',
          monero_address: xmrMoneroAddress.value.trim(),
          privacy: xmrPrivateListing.value ? 'private' : 'public',
          terms: xmrTerms.value ? 'confirm' : '',
        }
        extras.country = xmrCountry.value
        if (xmrCity.value.trim()) extras.city = xmrCity.value.trim()
      }

      // Strip HTML for XMR Bazaar descriptions (also stripped server-side)
      let description = formDescription.value.trim() || null
      if (isXmr.value && description) {
        description = stripHtml(description)
      }

      // Merge eBay extras into the extras object
      const finalExtras = { ...(extras || {}), ...(ebayExtras || {}) }

      if (!selectedVariant.value) throw new Error('Select a variant before publishing')

      await createListingOnPlatform({
        targetPlatform: selectedPlatform.value,
        productId,
        title: formTitle.value.trim(),
        descriptionHtml: description,
        priceAmount: formPriceAmount.value,
        priceCurrency: formPriceAmount.value !== null ? formPriceCurrency.value : null,
        photoUrls: formPhotos.value,
        extras: Object.keys(finalExtras).length > 0 ? finalExtras : undefined,
        variantId: selectedVariant.value.id,
        sku: selectedVariant.value.sku ?? null,
        quantity: selectedVariant.value.quantity ?? null,
      })
    }
    navigateTo(`/products/${productId}`)
  } catch (e: any) {
    publishError.value = e?.toString() || (isEditMode.value ? 'Failed to update listing' : 'Failed to publish listing')
  } finally {
    publishing.value = false
  }
}
</script>

<template>
  <CompanyRequired>
  <div class="flex flex-col h-full">
    <!-- Scrollable content -->
    <div class="flex-1 overflow-auto min-h-0 -mr-6 pr-6">
      <div class="pb-6">
      <div class="flex items-end justify-between mb-6">
        <div class="flex items-center gap-4">
          <NuxtLink :to="`/products/${productId}`" class="text-xs text-muted/40 hover:text-muted transition-colors">&larr; Back</NuxtLink>
          <h2 class="text-sm font-medium tracking-widest uppercase text-muted/60">{{ isEditMode ? 'Edit Listing' : 'Publish' }}</h2>
        </div>
        <div v-if="sourceListing" class="flex items-center gap-2 text-[11px] text-muted/30">
          Source <PlatformBadge :platform="sourceListing.platform" icon-only />
        </div>
      </div>

      <!-- Target platform (first question) -->
      <div v-if="!loading && !loadError">
        <h3 class="text-[11px] text-muted/40 uppercase tracking-wider mb-3">{{ isEditMode ? 'Platform' : 'Target platform' }}</h3>
        <div v-if="publishablePlatforms.length === 0 && !isEditMode" class="text-sm text-muted/40">
          No platforms available. All platforms with create support already have a mapping.
        </div>
        <TabBar
          v-else
          v-model="selectedPlatformModel"
          :items="platformTabItems"
          :disabled="isEditMode"
        />
      </div>

      <!-- Variant selector -->
      <div v-if="showVariantSelector && selectedPlatform && !loading && !loadError" class="mt-6">
        <h3 class="text-[11px] text-muted/40 uppercase tracking-wider mb-3">
          {{ platformSupportsMultiVariant ? 'Variants' : 'Variant' }}
        </h3>

        <!-- Multi-variant: attribute builder -->
        <div v-if="platformSupportsMultiVariant">
          <!-- Attribute select dropdowns + Add buttons -->
          <div v-if="variantAttributeKeys.length > 0" class="flex items-end gap-3 flex-wrap">
            <div v-for="key in variantAttributeKeys" :key="key" class="min-w-[120px]">
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1">{{ key }}</label>
              <Select
                :model-value="builderSelections[key] || ''"
                @update:model-value="onBuilderSelectionChange(key, $event)"
              >
                <SelectTrigger>
                  <SelectValue :placeholder="'--'" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="val in variantAttributeOptions[key]" :key="val" :value="val">{{ val }}</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <Button size="xs" color="accent" :disabled="!builderMatch || selectedVariantIds.includes(builderMatch?.id ?? '')" @click="addSelectedVariant">+ Add</Button>
            <Button size="xs" variant="ghost" @click="addAllVariants">Add All</Button>
          </div>
          <!-- No attributes — show a simple Add All -->
          <div v-else class="flex items-center gap-3">
            <span class="text-xs text-muted/40">{{ variants.length }} variants available</span>
            <Button size="xs" color="accent" @click="addAllVariants">Add All</Button>
          </div>

          <!-- Selected variant cards -->
          <div v-if="selectedVariantIds.length > 0" class="flex flex-wrap gap-2 mt-3">
            <GroundGlass v-for="v in selectedVariants" :key="v.id" :opacity="1.2" class="variant-card">
              <span class="relative z-[1]">{{ variantLabel(v) }}</span>
              <button class="variant-card-remove relative z-[1]" @click="removeVariant(v.id)">&times;</button>
            </GroundGlass>
          </div>
        </div>

        <!-- Single-variant: standard Select -->
        <div v-else class="max-w-xs">
          <Select v-model="singleVariantModel">
            <SelectTrigger>
              <SelectValue placeholder="Select variant" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem v-for="opt in variantSelectOptions" :key="opt.value" :value="opt.value">{{ opt.label }}</SelectItem>
            </SelectContent>
          </Select>
        </div>

        <!-- Selected variant summary (single variant info) -->
        <div v-if="!platformSupportsMultiVariant && selectedVariants.length === 1" class="mt-2 text-xs text-muted/60">
          <span class="font-mono text-foreground/80">{{ selectedVariants[0].sku }}</span>
          <span class="mx-1.5">&middot;</span>
          <span>{{ selectedVariants[0].name }}</span>
          <span class="mx-1.5">&middot;</span>
          <span>Qty: {{ selectedVariants[0].quantity }}</span>
        </div>

        <!-- Source selection -->
        <div v-if="selectedVariant && variantSources.length > 0" class="mt-3">
          <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Prefill from</label>
          <div class="flex flex-wrap gap-2">
            <button
              v-for="(src, i) in variantSources"
              :key="i"
              class="text-[11px] px-2.5 py-1 rounded border border-border/25 text-muted/70 hover:text-accent hover:border-accent/40 transition-colors"
              @click="applySource(src)"
            >{{ src.label }}</button>
          </div>
        </div>
      </div>
      </div>

    <!-- Loading / Error -->
    <div v-if="loading" class="py-16 text-sm text-muted/40">Loading listing data...</div>
    <div v-else-if="loadError" class="py-8">
      <p class="text-sm text-red-400/80">{{ loadError }}</p>
    </div>

    <!-- Variant required hint -->
    <div v-else-if="selectedPlatform && showVariantSelector && !variantSelectionComplete" class="py-16 text-sm text-muted/30">
      Select {{ platformSupportsMultiVariant ? 'one or more variants' : 'a variant' }} to continue.
    </div>

    <!-- Form -->
    <div v-else-if="selectedPlatform && variantSelectionComplete">
      <div class="max-w-2xl space-y-12">

        <!-- Title -->
        <section>
          <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Title</label>
          <Splatter trigger="focus" :opacity-ranges="[[0.08, 0.18], [0.08, 0.18], [0.03, 0.15]]">
            <input
              v-model="formTitle"
              class="relative z-[1] w-full bg-transparent border-b border-border/30 px-2 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors"
            >
          </Splatter>
        </section>
      </div>

      <!-- Description (full width) -->
      <section class="my-12">
        <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Description</label>
        <DescriptionEditor
          v-model="formDescription"
          :output-format="isXmr ? 'text' : 'html'"
          :disabled="false"
        />
      </section>

      <div class="max-w-2xl space-y-12 pb-8">
        <!-- Price -->
        <section>
          <h3 class="text-[11px] text-muted/40 uppercase tracking-wider mb-4">Price</h3>
          <div class="grid grid-cols-2 gap-x-8">
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Amount</label>
              <Splatter trigger="focus" :opacity-ranges="[[0.08, 0.18], [0.08, 0.18], [0.03, 0.15]]">
                <input
                  v-model.number="formPriceAmount"
                  type="number"
                  step="0.01"
                  placeholder="0.00"
                  class="relative z-[1] w-full bg-transparent border-b border-border/30 px-2 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors"
                >
              </Splatter>
            </div>
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Currency</label>
              <Select v-model="formPriceCurrency">
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  <template v-if="isXmr">
                    <SelectItem v-for="c in xmrCurrencies" :key="c.value" :value="c.value">{{ c.label }}</SelectItem>
                  </template>
                  <template v-else>
                    <SelectItem value="USD">USD</SelectItem>
                    <SelectItem value="EUR">EUR</SelectItem>
                    <SelectItem value="GBP">GBP</SelectItem>
                    <SelectItem value="XMR">XMR</SelectItem>
                  </template>
                </SelectContent>
              </Select>
            </div>
          </div>
        </section>

        <!-- Photos -->
        <section>
          <h3 class="text-[11px] text-muted/40 uppercase tracking-wider mb-4">Photos ({{ formPhotos.length }})</h3>
          <div v-if="formPhotos.length" class="flex gap-2 flex-wrap mb-4">
            <div
              v-for="(url, i) in formPhotos"
              :key="url"
              class="relative shrink-0 w-20 h-20 rounded-lg overflow-hidden bg-surface/30 group"
            >
              <img :src="resolveThumb(url) ?? url" :alt="`Photo ${i + 1}`" class="w-full h-full object-cover" loading="lazy">
              <button
                v-if="!isEditMode"
                class="absolute top-0.5 right-0.5 w-5 h-5 rounded-full bg-black/70 text-white text-xs flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity"
                @click="removePhoto(i)"
              >
                &times;
              </button>
            </div>
          </div>
          <div v-if="!isEditMode" class="flex items-center gap-3">
            <Splatter trigger="focus" :opacity-ranges="[[0.08, 0.18], [0.08, 0.18], [0.03, 0.15]]" class="flex-1">
              <input
                v-model="newPhotoUrl"
                type="text"
                placeholder="Add photo URL..."
                class="relative z-[1] w-full bg-transparent border-b border-border/30 px-2 py-1 text-sm text-foreground placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
                @keydown.enter.prevent="addPhoto"
              >
            </Splatter>
            <Button
              size="xs"
              color="accent"
              :disabled="!newPhotoUrl.trim()"
              @click="addPhoto"
            >
              Add
            </Button>
          </div>
          <p v-else-if="formPhotos.length === 0" class="text-xs text-muted/30">No photos on this listing.</p>
        </section>

        <!-- eBay: Category -->
        <section v-if="isEbay">
          <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">eBay Category</h3>
          <div v-if="ebayCategoryId" class="flex items-center gap-3 mb-4">
            <span class="text-sm text-foreground">{{ ebayCategoryName }}</span>
            <span class="text-[11px] text-muted/30">ID: {{ ebayCategoryId }}</span>
            <button
              class="text-xs text-muted/40 hover:text-red-400 transition-colors"
              @click="clearEbayCategory"
            >
              Clear
            </button>
          </div>
          <div v-else class="relative">
            <Splatter trigger="focus" :opacity-ranges="[[0.08, 0.18], [0.08, 0.18], [0.03, 0.15]]">
              <input
                v-model="ebayCategoryQuery"
                type="text"
                placeholder="Search for a category..."
                class="relative z-[1] w-full bg-transparent border-b border-border/30 px-2 py-1.5 text-sm text-foreground placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
                @input="onEbayCategoryInput"
              >
            </Splatter>
            <p v-if="ebayCategoryLoading" class="text-[11px] text-muted/30 mt-1">Searching...</p>
            <GroundGlass v-if="ebayCategorySuggestions.length > 0" :opacity="2.5" class="ebay-cat-dropdown">
              <div class="ebay-cat-dropdown__list">
                <button
                  v-for="suggestion in ebayCategorySuggestions"
                  :key="suggestion.category.category_id"
                  class="ebay-cat-dropdown__item"
                  @click="selectEbayCategory(suggestion)"
                >
                  <div class="ebay-cat-dropdown__name">{{ suggestion.category.category_name }}</div>
                  <div class="ebay-cat-dropdown__path">{{ getCategoryBreadcrumb(suggestion) }}</div>
                </button>
              </div>
            </GroundGlass>
            <p v-if="!isEditMode" class="text-[11px] text-muted/30 mt-1">Required for eBay listings.</p>
          </div>
        </section>

        <!-- eBay: Item Specifics -->
        <section v-if="isEbay && ebayCategoryId && ebayAspectMetadata.length > 0">
          <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">Item Specifics</h3>
          <p v-if="ebayAspectsLoading" class="text-sm text-muted/30">Loading aspects...</p>
          <div v-else class="space-y-4">
            <div
              v-for="aspect in [...ebayAspectMetadata].sort((a, b) => {
                const aReq = a.constraint?.required ? 0 : 1
                const bReq = b.constraint?.required ? 0 : 1
                return aReq - bReq
              })"
              :key="aspect.name"
            >
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">
                {{ aspect.name }}
                <span v-if="aspect.constraint?.required" class="text-red-400/70">*</span>
              </label>
              <!-- Known values + MULTI → MultiSelect -->
              <MultiSelect
                v-if="aspect.values?.length && aspect.constraint?.cardinality === 'MULTI'"
                :model-value="ebayAspects[aspect.name] || []"
                :options="aspect.values.map(v => ({ label: v.value, value: v.value }))"
                :placeholder="aspect.constraint?.required ? 'Required' : 'Optional'"
                filterable
                @update:model-value="ebayAspects[aspect.name] = $event"
              />
              <!-- Known values + SINGLE → Select -->
              <Select
                v-else-if="aspect.values?.length"
                :model-value="(ebayAspects[aspect.name] || [])[0] || undefined"
                @update:model-value="setAspectSingleValue(aspect.name, $event)"
              >
                <SelectTrigger
                  :clearable="!!(ebayAspects[aspect.name]?.length)"
                  @clear="setAspectSingleValue(aspect.name, '')"
                >
                  <SelectValue :placeholder="aspect.constraint?.required ? 'Required' : 'Optional'" />
                </SelectTrigger>
                <SelectContent filterable>
                  <SelectItem v-for="v in aspect.values" :key="v.value" :value="v.value" :text-value="v.value">{{ v.value }}</SelectItem>
                </SelectContent>
              </Select>
              <!-- No known values + MULTI → TagInput -->
              <template v-else-if="aspect.constraint?.cardinality === 'MULTI'">
                <TagInput
                  :model-value="getAspectValue(aspect.name)"
                  :placeholder="aspect.constraint?.required ? 'Required' : 'Optional'"
                  @update:model-value="setAspectValue(aspect.name, $event)"
                />
                <p class="text-[11px] text-muted/20 mt-0.5">Press Enter or comma to add multiple values</p>
              </template>
              <!-- No known values + SINGLE → text input -->
              <Splatter v-else trigger="focus" :opacity-ranges="[[0.08, 0.18], [0.08, 0.18], [0.03, 0.15]]">
                <input
                  :value="(ebayAspects[aspect.name] || [])[0] || ''"
                  :placeholder="aspect.constraint?.required ? 'Required' : 'Optional'"
                  class="relative z-[1] w-full bg-transparent border-b border-border/30 px-2 py-1.5 text-sm text-foreground placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
                  @input="setAspectSingleValue(aspect.name, ($event.target as HTMLInputElement).value)"
                >
              </Splatter>
            </div>
          </div>
        </section>

        <!-- eBay: Condition -->
        <section v-if="isEbay">
          <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">Condition</h3>
          <Select v-model="ebayCondition">
            <SelectTrigger><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem v-for="c in ebayConditions" :key="c.value" :value="c.value">{{ c.label }}</SelectItem>
            </SelectContent>
          </Select>
          <div v-if="ebayCondition !== 'NEW'" class="mt-4">
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Condition Description</label>
            <Splatter trigger="focus" :opacity-ranges="[[0.08, 0.18], [0.08, 0.18], [0.03, 0.15]]">
              <textarea
                v-model="ebayConditionDescription"
                rows="3"
                placeholder="Describe the condition of the item..."
                class="relative z-[1] w-full bg-transparent border border-border/20 rounded px-3 py-2 text-sm text-foreground focus:outline-none focus:border-accent/30 transition-colors resize-y"
              />
            </Splatter>
          </div>
        </section>

        <!-- eBay: Business Policies -->
        <section v-if="isEbay">
          <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">Business Policies</h3>
          <p v-if="ebayPoliciesLoading" class="text-sm text-muted/30">Loading policies...</p>
          <p v-else-if="ebayPoliciesError" class="text-xs text-red-400/70 mb-4">{{ ebayPoliciesError }}</p>
          <div v-else class="space-y-4">
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Fulfillment Policy</label>
              <Select v-if="ebayFulfillmentPolicies.length > 0" v-model="ebayFulfillmentPolicyId">
                <SelectTrigger :clearable="!!ebayFulfillmentPolicyId" @clear="ebayFulfillmentPolicyId = ''">
                  <SelectValue placeholder="-- Select --" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="p in ebayFulfillmentPolicies" :key="p.id" :value="p.id">{{ p.name || p.id }}</SelectItem>
                </SelectContent>
              </Select>
              <p v-else class="text-xs text-muted/30">No fulfillment policies found. Create one in eBay Seller Hub.</p>
            </div>
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Payment Policy</label>
              <Select v-if="ebayPaymentPolicies.length > 0" v-model="ebayPaymentPolicyId">
                <SelectTrigger :clearable="!!ebayPaymentPolicyId" @clear="ebayPaymentPolicyId = ''">
                  <SelectValue placeholder="-- Select --" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="p in ebayPaymentPolicies" :key="p.id" :value="p.id">{{ p.name || p.id }}</SelectItem>
                </SelectContent>
              </Select>
              <p v-else class="text-xs text-muted/30">No payment policies found. Create one in eBay Seller Hub.</p>
            </div>
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Return Policy</label>
              <Select v-if="ebayReturnPolicies.length > 0" v-model="ebayReturnPolicyId">
                <SelectTrigger :clearable="!!ebayReturnPolicyId" @clear="ebayReturnPolicyId = ''">
                  <SelectValue placeholder="-- Select --" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="p in ebayReturnPolicies" :key="p.id" :value="p.id">{{ p.name || p.id }}</SelectItem>
                </SelectContent>
              </Select>
              <p v-else class="text-xs text-muted/30">No return policies found. Create one in eBay Seller Hub.</p>
            </div>
          </div>
        </section>

        <!-- XMR: Listing Details -->
        <section v-if="isXmr">
          <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">Listing details</h3>
          <div class="grid grid-cols-2 gap-x-8 gap-y-5">
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Category</label>
              <Select v-model="xmrCategory">
                <SelectTrigger><SelectValue /></SelectTrigger>
                <SelectContent>
                  <SelectItem v-for="c in xmrCategories" :key="c.value" :value="c.value">{{ c.label }}</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Tags</label>
              <TagInput v-model="xmrTags" placeholder="Add a tag..." />
            </div>
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Stock</label>
              <TabBar v-model="xmrStock" :items="[
                { label: 'Unlimited', value: 'unlimited' },
                { label: 'One-time', value: 'one-time' },
              ]" />
            </div>
          </div>
          <div class="flex items-center gap-6 mt-5">
            <label class="flex items-center gap-2.5 text-sm text-foreground/70 cursor-pointer" @click.prevent>
              <Switch v-model="xmrFlexiblePrice" /> Flexible price
            </label>
            <label class="flex items-center gap-2.5 text-sm text-foreground/70 cursor-pointer" @click.prevent>
              <Switch v-model="xmrPrivateListing" /> Private listing
            </label>
          </div>
        </section>

        <!-- XMR: Delivery & Location -->
        <section v-if="isXmr">
          <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">Delivery & location</h3>
          <div class="space-y-5">
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Delivery type</label>
              <TabBar v-model="xmrDelivery" :items="[
                { label: 'Digital', value: 'digital' },
                { label: 'Physical', value: 'physical' },
              ]" />
            </div>
            <div class="grid grid-cols-2 gap-x-8">
              <div>
                <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Country</label>
                <Select v-model="xmrCountry">
                  <SelectTrigger><SelectValue /></SelectTrigger>
                  <SelectContent>
                    <SelectItem v-for="c in xmrCountries" :key="c.value" :value="c.value">{{ c.label }}</SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div>
                <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">City</label>
                <Splatter trigger="focus" :opacity-ranges="[[0.08, 0.18], [0.08, 0.18], [0.03, 0.15]]">
                  <input
                    v-model="xmrCity"
                    placeholder="Optional"
                    class="relative z-[1] w-full bg-transparent border-b border-border/30 px-2 py-1.5 text-sm text-foreground placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
                  >
                </Splatter>
              </div>
            </div>
            <div class="flex items-center gap-4">
              <label class="flex items-center gap-2.5 text-sm text-foreground/70 cursor-pointer" @click.prevent>
                <Switch v-model="xmrInPersonPickup" /> In-person pickup
              </label>
              <Splatter v-if="xmrInPersonPickup" trigger="focus" class="flex-1">
                <input
                  v-model="xmrPickupShippingCost"
                  type="text"
                  placeholder="Cost (optional)"
                  class="relative z-[1] w-full bg-transparent border-b border-border/30 px-2 py-1 text-sm text-foreground placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
                >
              </Splatter>
            </div>
          </div>
        </section>

        <!-- XMR: Shipping -->
        <section v-if="isXmr">
          <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">Shipping</h3>
          <div class="space-y-4">
            <div class="flex items-center gap-4">
              <label class="flex items-center gap-2.5 text-sm text-foreground/70 cursor-pointer" @click.prevent>
                <Switch v-model="xmrDomesticShipping" /> Domestic
              </label>
              <div v-if="xmrDomesticShipping" class="flex items-center gap-1.5">
                <span class="text-xs text-muted/30">$</span>
                <Splatter trigger="focus" :opacity-ranges="[[0.08, 0.18], [0.08, 0.18], [0.03, 0.15]]">
                  <input
                    v-model="xmrDomesticShippingCost"
                    type="text"
                    placeholder="Cost"
                    class="relative z-[1] w-24 bg-transparent border-b border-border/30 px-2 py-1 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors"
                  >
                </Splatter>
              </div>
            </div>
            <div class="flex items-center gap-4">
              <label class="flex items-center gap-2.5 text-sm text-foreground/70 cursor-pointer" @click.prevent>
                <Switch v-model="xmrInternationalShipping" /> International
              </label>
              <div v-if="xmrInternationalShipping" class="flex items-center gap-1.5">
                <span class="text-xs text-muted/30">$</span>
                <Splatter trigger="focus" :opacity-ranges="[[0.08, 0.18], [0.08, 0.18], [0.03, 0.15]]">
                  <input
                    v-model="xmrInternationalShippingCost"
                    type="text"
                    placeholder="Cost"
                    class="relative z-[1] w-24 bg-transparent border-b border-border/30 px-2 py-1 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors"
                  >
                </Splatter>
              </div>
            </div>
          </div>
        </section>

        <!-- XMR: Payment -->
        <section v-if="isXmr">
          <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">Payment</h3>
          <div class="space-y-5">
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Method</label>
              <TabBar v-model="xmrPaymentMethod" :items="[
                { label: 'Direct', value: 'direct' },
                { label: 'Escrow', value: 'escrow' },
              ]" />
            </div>
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Monero address</label>
              <Splatter trigger="focus" :opacity-ranges="[[0.08, 0.18], [0.08, 0.18], [0.03, 0.15]]">
                <input
                  v-model="xmrMoneroAddress"
                  placeholder="Your Monero wallet address"
                  class="relative z-[1] w-full bg-transparent border-b border-border/30 px-2 py-1.5 text-sm text-foreground font-mono placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
                >
              </Splatter>
              <p v-if="!xmrMoneroAddress.trim()" class="text-[11px] text-red-400/70 mt-1">Required</p>
            </div>
            <label class="flex items-center gap-2.5 text-sm text-foreground/70 cursor-pointer" @click.prevent>
              <Switch v-model="xmrTerms" /> I agree to the terms
            </label>
          </div>
        </section>

        <!-- Errors + Publish -->
        <section>
          <p v-if="isEbay && !ebayValid" class="text-xs text-warning/70 mb-4">
            <span v-if="!isEditMode && !ebayCategoryId">eBay category is required. </span>
            <span v-for="aspect in ebayAspectMetadata.filter(a => a.constraint?.required && (!ebayAspects[a.name] || ebayAspects[a.name].length === 0))" :key="aspect.name">{{ aspect.name }} is required. </span>
          </p>
          <p v-if="isXmr && !xmrValid" class="text-xs text-warning/70 mb-4">
            <span v-if="!xmrMoneroAddress.trim()">Monero address is required. </span>
            <span v-if="!xmrTerms">You must agree to the terms.</span>
          </p>
          <p v-if="publishError" class="text-xs text-red-400/80 mb-4">{{ publishError }}</p>
          <Button
            size="sm"
            color="accent"
            :disabled="!canPublish || publishing"
            @click="handlePublish"
          >
            {{ publishing ? (isEditMode ? 'Updating...' : 'Publishing...') : (isEditMode ? 'Update listing' : 'Publish listing') }}
          </Button>
        </section>
      </div>
    </div>

    <!-- No platform selected -->
    <div v-else-if="publishablePlatforms.length > 0" class="py-16 text-sm text-muted/30">
      Select a target platform to continue.
    </div>
    </div>
  </div>
  </CompanyRequired>
</template>

<style scoped>
/* ════ Variant Builder ════ */
.variant-card {
  display: inline-flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.25rem 0.5rem 0.25rem 0.75rem;
  font-size: 0.75rem;
  color: var(--foreground);
  border-radius: 0.5rem;
}

.variant-card-remove {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  font-size: 14px;
  line-height: 1;
  color: rgba(148,163,184,0.5);
  transition: color 0.12s ease, background 0.12s ease;
  cursor: pointer;
  background: transparent;
  border: none;
}
.variant-card-remove:hover {
  color: #f87171;
  background: rgba(248,113,113,0.1);
}

.ebay-cat-dropdown {
  margin-top: 8px;
  max-height: 280px;
  padding: 4px;
}

.ebay-cat-dropdown__list {
  position: relative;
  z-index: 1;
  overflow-y: auto;
  max-height: 272px;
}

.ebay-cat-dropdown__item {
  display: block;
  width: 100%;
  text-align: left;
  padding: 8px 10px;
  border-radius: 0.375rem;
  cursor: pointer;
  outline: none;
  background: transparent;
  border: none;
  transition: background 0.1s ease, color 0.1s ease;
}

.ebay-cat-dropdown__item:hover {
  background: rgba(139, 92, 246, 0.12);
}

.ebay-cat-dropdown__name {
  font-size: 13px;
  font-weight: 500;
  color: #e2e8f0;
}

.ebay-cat-dropdown__path {
  font-size: 11px;
  color: rgba(148, 163, 184, 0.45);
  margin-top: 1px;
}

.ebay-cat-dropdown__item:hover .ebay-cat-dropdown__path {
  color: rgba(148, 163, 184, 0.65);
}
</style>
