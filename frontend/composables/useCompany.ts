import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { CompanyInfo, CompanyPeer, P2PEvent } from '~/types'

export function useCompany() {
  const company = ref<CompanyInfo | null>(null)
  const loading = ref(false)
  const onionAddress = ref<string | null>(null)
  const syncing = ref(false)
  const { fetchCompanies } = useCompanyContext()

  async function fetchCompany() {
    loading.value = true
    try {
      company.value = await invoke<CompanyInfo>('get_company')
    } catch {
      company.value = null
    } finally {
      loading.value = false
    }
  }

  async function createCompany(name: string) {
    loading.value = true
    try {
      company.value = await invoke<CompanyInfo>('create_company', { name })
      // Refresh company list but don't update activeCompanyId on the frontend yet.
      // The backend already set the active company, but changing the frontend ref
      // would trigger app.vue's page re-mount, destroying any in-progress wizard/form.
      await fetchCompanies({ skipActiveId: true })
    } finally {
      loading.value = false
    }
  }

  async function joinCompany(adminOnion: string, secret: string, name: string) {
    loading.value = true
    try {
      company.value = await invoke<CompanyInfo>('join_company', { adminOnion, secret, name })
      await fetchCompanies()
    } finally {
      loading.value = false
    }
  }

  async function leaveCompany() {
    await invoke('leave_company', {})
    company.value = null
    onionAddress.value = null
    await fetchCompanies()
  }

  async function getOnionAddress(): Promise<string | null> {
    const addr = await invoke<string | null>('get_onion_address')
    onionAddress.value = addr
    return addr
  }

  async function getCompanySecret(): Promise<string | null> {
    return invoke<string | null>('get_company_secret')
  }

  async function addPeer(onionAddr: string, name: string) {
    await invoke('add_company_peer', { onionAddress: onionAddr, name })
    await fetchCompany()
  }

  async function removePeer(onionAddr: string) {
    await invoke('remove_company_peer', { onionAddress: onionAddr })
    await fetchCompany()
  }

  async function approvePeer(onionAddr: string) {
    await invoke('approve_company_peer', { onionAddress: onionAddr })
    await fetchCompany()
  }

  async function rejectPeer(onionAddr: string) {
    await invoke('reject_company_peer', { onionAddress: onionAddr })
    await fetchCompany()
  }

  async function setPeerRole(onionAddr: string, role: 'admin' | 'member') {
    await invoke('set_peer_role', { onionAddress: onionAddr, role })
    await fetchCompany()
  }

  async function updateLogo(dataUrl: string) {
    await invoke('update_company_logo', { logo: dataUrl })
    await fetchCompany()
  }

  async function removeLogo() {
    await invoke('remove_company_logo')
    await fetchCompany()
  }

  async function triggerSync() {
    syncing.value = true
    try {
      await invoke('trigger_p2p_sync')
    } finally {
      setTimeout(() => { syncing.value = false }, 2000)
    }
  }

  function listenForEvents(callback?: (event: P2PEvent) => void) {
    return listen<P2PEvent>('p2p-event', (e) => {
      const event = e.payload

      if (event.type === 'OnionServiceReady') {
        onionAddress.value = event.onion_address
      }

      if (event.type === 'SyncCompleted' || event.type === 'SyncFailed') {
        syncing.value = false
        fetchCompany()
      }

      if (event.type === 'PeerJoinRequest') {
        fetchCompany()
      }

      callback?.(event)
    })
  }

  return {
    company,
    loading,
    onionAddress,
    syncing,
    fetchCompany,
    createCompany,
    joinCompany,
    leaveCompany,
    getOnionAddress,
    getCompanySecret,
    addPeer,
    removePeer,
    approvePeer,
    rejectPeer,
    setPeerRole,
    updateLogo,
    removeLogo,
    triggerSync,
    listenForEvents,
  }
}
