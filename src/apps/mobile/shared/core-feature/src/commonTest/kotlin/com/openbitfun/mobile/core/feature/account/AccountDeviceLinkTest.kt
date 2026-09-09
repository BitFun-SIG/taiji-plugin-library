package com.openbitfun.mobile.core.feature.account

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

class AccountDeviceLinkTest {
    private val link = "https://remote.openbitfun.com/v/1.0.0/#/pair?did=desktop-1&pk=untrusted&relay=https://evil.example"
    private val ready = AccountUiState.Ready("user", "name", listOf(AccountDeviceUi("desktop-1", "Desktop", true, null)), null, null)

    @Test fun onlyAuthenticatedOnlineMembershipCanResolveTheTarget() {
        assertEquals(AccountDeviceLinkStatus.SIGN_IN_REQUIRED, resolveAccountDeviceLink(link, AccountUiState.SignedOut).status)
        assertEquals("desktop-1", resolveAccountDeviceLink(link, ready).deviceId)
        assertEquals(AccountDeviceLinkStatus.UNAVAILABLE, resolveAccountDeviceLink(link.replace("desktop-1", "foreign"), ready).status)
        assertEquals(AccountDeviceLinkStatus.UNAVAILABLE, resolveAccountDeviceLink(link, ready.copy(devices = ready.devices.map { it.copy(online = false) })).status)
    }

    @Test fun rejectsLegacyAndLookalikeLinksWithoutUsingTheirRoutingFields() {
        for (invalid in listOf(
            link.replace("https:", "http:"), link.replace("remote.openbitfun.com", "evil.example"),
            link.replace("remote.openbitfun.com", "user@remote.openbitfun.com"),
            link.replace("/v/1.0.0/", "/relay/"), link + "&did=foreign", link.replace("did=desktop-1", "room=old"),
            link.replace("did=desktop-1", "did=../bad"),
        )) {
            val result = resolveAccountDeviceLink(invalid, ready)
            assertEquals(AccountDeviceLinkStatus.INVALID, result.status, invalid)
            assertNull(result.deviceId)
        }
    }
}
