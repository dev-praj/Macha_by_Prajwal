package com.macha.android.core

/**
 * Android's current supported app language. The translation catalog is retained in source for
 * future use, but the shipped phone app is intentionally English-only.
 */
object Languages {
    data class Language(val tag: String, val name: String)

    const val SYSTEM = "en"

    val ALL = emptyList<Language>()

    /** The tag of the language that [tag] (as Android reports it, e.g. "zh-Hans-CN" or "pt-PT") stands for in the app; [SYSTEM] if there is none. */
    fun match(@Suppress("UNUSED_PARAMETER") tag: String?): String = SYSTEM
}
