package com.openbitfun.mobile.core.transport

import io.ktor.http.Url

/** QR fields are untrusted hints. Only the official origin and a device ID survive. */
public fun accountDeviceIdFromLink(value: String): String? = runCatching {
    if (value.length > 8192) return null
    val url = Url(value.trim())
    if (url.protocol.name != "https" || url.host != "remote.openbitfun.com" ||
        url.port != 443 || url.user != null || url.password != null ||
        url.encodedPath !in setOf("/v/1.0.0", "/v/1.0.0/") ||
        !url.fragment.startsWith("/pair?")) return null
    val query = Url("https://remote.openbitfun.com/?" + url.fragment.removePrefix("/pair?"))
    val ids = query.parameters.getAll("did") ?: return null
    ids.singleOrNull()?.takeIf { id ->
        id.isNotEmpty() && id.length <= 128 && id.all { it.isLetterOrDigit() && it.code < 128 || it in "-_.:" }
    }
}.getOrNull()
