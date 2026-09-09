package com.openbitfun.mobile.core.feature.account

import com.openbitfun.mobile.core.transport.accountDeviceIdFromLink

public enum class AccountDeviceLinkStatus { INVALID, SIGN_IN_REQUIRED, UNAVAILABLE, READY }

public data class AccountDeviceLinkResult(public val status: AccountDeviceLinkStatus, public val deviceId: String?)

/** Resolves against authenticated membership; the QR never supplies credentials or a relay URL. */
public fun resolveAccountDeviceLink(url: String, state: AccountUiState): AccountDeviceLinkResult {
    val id = accountDeviceIdFromLink(url)
        ?: return AccountDeviceLinkResult(AccountDeviceLinkStatus.INVALID, null)
    val ready = state as? AccountUiState.Ready
        ?: return AccountDeviceLinkResult(AccountDeviceLinkStatus.SIGN_IN_REQUIRED, id)
    return if (ready.devices.any { it.id == id && it.online }) {
        AccountDeviceLinkResult(AccountDeviceLinkStatus.READY, id)
    } else AccountDeviceLinkResult(AccountDeviceLinkStatus.UNAVAILABLE, null)
}
