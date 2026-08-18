import type { Settings } from '../api/tauri'
import { isWebSearchConfigured } from '../settings/webSearch'

export function providerHasUsableConfig(settings: Settings): boolean {
  return settings.providers.some((provider) =>
    provider.enabled !== false
    && provider.apiKeys.some((key) => key.trim() !== '')
    && provider.enabledModels.length > 0,
  )
}

export function isProviderModelBindingUsable(
  settings: Settings,
  providerId: string,
  model: string,
): boolean {
  const id = providerId.trim()
  const modelName = model.trim()
  if (!id || !modelName) return false

  const provider = settings.providers.find((item) => item.id === id)
  if (!provider || provider.enabled === false) return false
  if (!provider.baseUrl || provider.baseUrl.trim() === '') return false
  if (!provider.apiKeys.some((key) => key.trim() !== '')) return false
  if (provider.enabledModels.length === 0) return false
  return provider.enabledModels.includes(modelName)
}

export function validateProviderStep(settings: Settings): { ok: boolean; reason?: string } {
  if (settings.providers.length === 0) {
    return { ok: false, reason: 'no_provider' }
  }

  const quickProviderId = settings.screenshotTranslation?.providerId?.trim() ?? ''
  const quickModel = settings.screenshotTranslation?.model?.trim() ?? ''
  if (!isProviderModelBindingUsable(settings, quickProviderId, quickModel)) {
    return { ok: false, reason: 'missing_quick_translate_model' }
  }

  const lensProviderId = settings.lens?.providerId?.trim() ?? ''
  const lensModel = settings.lens?.model?.trim() ?? ''
  if (!isProviderModelBindingUsable(settings, lensProviderId, lensModel)) {
    return { ok: false, reason: 'missing_lens_model' }
  }

  const chatProviderId = settings.defaultModels.chat.providerId.trim()
  const chatModel = settings.defaultModels.chat.model.trim()
  if (!isProviderModelBindingUsable(settings, chatProviderId, chatModel)) {
    return { ok: false, reason: 'missing_chat_model' }
  }

  return { ok: true }
}

export function canCompleteOnboarding(settings: Settings): boolean {
  return validateProviderStep(settings).ok
}

export function webSearchConfigured(settings: Settings): boolean {
  return isWebSearchConfigured(settings.lens?.webSearch)
}
