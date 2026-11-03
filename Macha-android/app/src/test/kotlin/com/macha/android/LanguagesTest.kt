package com.macha.android

import com.macha.android.core.Languages
import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Android ships English only, independently of device locale and old saved per-app language choices. */
class LanguagesTest {
    private fun read(path: String) = File(path).readText()

    /** The language table of the generator: tag, folder qualifier and name. */
    private val table = Regex("""\{ tag: "([^"]+)", qualifier: "([^"]+)", name: "([^"]+)", plural: \[([^\]]*)\] \}""")
        .findAll(read("../scripts/gen-strings.mjs")).map { Triple(it.groupValues[1], it.groupValues[2], it.groupValues[3]) }.toList()
    private val translations = File("../i18n/translations").listFiles { f -> f.extension == "json" }.orEmpty().map { it.nameWithoutExtension }.sorted()

    @Test fun theGeneratorKnowsNineLanguages() {
        assertEquals(listOf("fr", "es", "pt-BR", "zh-Hans", "hi", "bn", "ru", "id", "ar"), table.map { it.first })
    }

    @Test fun theAppDoesNotOfferLanguageChoices() {
        assertTrue(Languages.ALL.isEmpty())
        assertEquals("en", Languages.match("fr"))
    }

    @Test fun androidsOwnLanguageListMatches() {
        val tags = Regex("""android:name="([^"]+)"""").findAll(read("src/main/res/xml/locales_config.xml")).map { it.groupValues[1] }.toList()
        assertEquals(listOf("en"), tags)
        assertTrue(read("src/main/AndroidManifest.xml").contains("""android:localeConfig="@xml/locales_config""""))
    }

    @Test fun theBuildKeepsOnlyEnglish() {
        val configured = Regex("""resourceConfigurations\s*\+=\s*listOf\(([^)]*)\)""").find(read("build.gradle.kts"))!!.groupValues[1]
            .split(",").map { it.trim().trim('"') }
        assertEquals(listOf("en"), configured)
    }

    @Test fun everyTranslationHasItsFolderAndNothingElseIsThere() {
        val folders = File("src/main/res").listFiles().orEmpty().map { it.name }.filter { it.startsWith("values") }.sorted()
        val expected = (listOf("values", "values-notnight") + table.filter { it.first in translations }.map { "values-${it.second}" }).sorted()
        assertEquals(expected, folders)
        for (f in expected.filter { it != "values-notnight" }) assertTrue(f, File("src/main/res/$f/strings.xml").exists())
        // values-notnight is the light variant of the Widgets' colours, and holds colours only.
        assertEquals(listOf("colors.xml"), File("src/main/res/values-notnight").listFiles().orEmpty().map { it.name })
    }

    @Test fun englishIsTheSingleSourceOfTheKeys() {
        val english = Regex("""<(?:string|plurals) name="([a-z_0-9]+)"""").findAll(read("src/main/res/values/strings.xml")).map { it.groupValues[1] }.toSet()
        val source = read("../i18n/app-strings.json")
        val declared = Regex("""^ {4}"([a-z_0-9]+)":""", RegexOption.MULTILINE).findAll(source).map { it.groupValues[1] }.toSet()
        assertEquals(declared, english)
    }

    @Test fun anyStoredOrSystemLanguageResolvesToEnglish() {
        assertEquals("en", Languages.match("fr"))
        assertEquals("en", Languages.match("fr-CA"))
        assertEquals("en", Languages.match("pt-PT"))
        assertEquals("en", Languages.match("PT-br"))
        assertEquals("en", Languages.match("de"))
        assertEquals(Languages.SYSTEM, Languages.match(""))
        assertEquals(Languages.SYSTEM, Languages.match(null))
    }

    @Test fun layoutsMirrorForRightToLeftLanguages() {
        assertTrue(read("src/main/AndroidManifest.xml").contains("""android:supportsRtl="true""""))
        // No layout or drawable says left or right: start and end mirror by themselves.
        for (dir in listOf("src/main/res/layout", "src/main/res/drawable")) for (f in File(dir).listFiles().orEmpty()) {
            val text = f.readText()
            for (attr in listOf("layout_marginLeft", "layout_marginRight", "paddingLeft", "paddingRight", "layout_alignParentLeft", "layout_alignParentRight", "drawableLeft", "drawableRight")) {
                assertFalse("${f.name} uses $attr", text.contains(attr))
            }
        }
        for (f in File("src/main/kotlin").walkTopDown().filter { it.extension == "kt" }) {
            val text = f.readText()
            for (bad in listOf("Alignment.Left", "Alignment.Right", "TextAlign.Left", "TextAlign.Right", "absolutePadding", "Arrangement.Left", "Arrangement.Right")) {
                assertFalse("${f.name} uses $bad", text.contains(bad))
            }
        }
    }
}
