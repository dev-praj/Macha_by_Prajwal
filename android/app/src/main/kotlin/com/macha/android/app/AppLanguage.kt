package com.macha.android.app

import android.app.LocaleManager
import android.content.Context
import android.content.res.Configuration
import android.os.Build
import android.os.LocaleList
import com.macha.android.core.Languages
import com.macha.android.core.Texts
import java.util.Locale

/**
 * Pins the Android app to English regardless of the device language or old per-app language choice.
 */
object AppLanguage {
    private const val PREFS = "macha_ui"
    private const val KEY = "app_language"

    fun current(context: Context): String = Languages.SYSTEM

    /** Enforces English even if an older version stored another language. */
    fun set(context: Context, @Suppress("UNUSED_PARAMETER") tag: String) {
        enforceEnglish(context)
    }

    private fun enforceEnglish(context: Context) {
        if (Build.VERSION.SDK_INT >= 33) {
            val manager = context.getSystemService(LocaleManager::class.java)
            if (manager.applicationLocales.toLanguageTags() != Languages.SYSTEM) {
                manager.applicationLocales = LocaleList.forLanguageTags(Languages.SYSTEM)
            }
        } else {
            context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(KEY, Languages.SYSTEM).apply()
        }
    }

    /** Applies the English locale before Android creates activity/application resources. */
    fun wrap(base: Context): Context {
        enforceEnglish(base)
        if (Build.VERSION.SDK_INT >= 33) return base
        Locale.setDefault(Locale.ENGLISH)
        val config = Configuration(base.resources.configuration).apply {
            setLocale(Locale.ENGLISH)
            setLayoutDirection(Locale.ENGLISH)
        }
        return base.createConfigurationContext(config)
    }

    /** The texts of the pure rules ([Texts]), looked up in the string resources of the application, in whatever language is in force. */
    fun texts(app: Context): Texts.Source = object : Texts.Source {
        override fun string(key: String): String? = TextIds.strings[key]?.let { app.resources.getString(it) }
        override fun plural(key: String, count: Int): String? = TextIds.plurals[key]?.let { app.resources.getQuantityString(it, count) }
    }
}
