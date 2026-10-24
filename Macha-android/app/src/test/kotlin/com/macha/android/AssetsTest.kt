package com.macha.android

import com.macha.android.macha.MachaConst
import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test

class AssetsTest {
    private val raw = File("src/main/res/raw")
    private val res = File("src/main/res")

    @Test fun everyBundledSoundIsTheDesktopsOwnFileByteForByte() {
        val desktopFiles = ReferenceFiles.file("windows/assets/sounds").listFiles { f -> f.extension == "wav" }
        assumeTrue("Macha desktop sound assets are not present", desktopFiles != null)
        val desktop = desktopFiles!!.associateBy { it.name }
        for (f in raw.listFiles()!!) {
            val original = desktop[f.name] ?: error("${f.name} is not one of the desktop's sounds")
            assertTrue(f.name, original.readBytes().contentEquals(f.readBytes()))
        }
    }

    @Test fun everySoundTheAppPlaysIsBundled() {
        val code = File("src/main/kotlin").walkTopDown().filter { it.extension == "kt" }.joinToString("\n") { it.readText() }
        val played = Regex("""\.play\("(\w+)"\)""").findAll(code).map { it.groupValues[1] }.toSet() + MachaConst.STATE_SOUND.values
        assertTrue(played.isNotEmpty())
        played.forEach { assertTrue("$it.wav", File(raw, "$it.wav").exists()) }
    }

    @Test fun noBundledSoundIsLeftUnused() {
        val code = File("src/main/kotlin").walkTopDown().filter { it.extension == "kt" && it.name != "SoundPlayer.kt" }.joinToString("\n") { it.readText() }
        val played = Regex("""\.play\("(\w+)"\)""").findAll(code).map { it.groupValues[1] }.toSet() + MachaConst.STATE_SOUND.values
        assertEquals(raw.listFiles()!!.map { it.nameWithoutExtension }.toSet(), played)
    }


    @Test fun rawNamesAreValidAndroidResourceNames() {
        raw.listFiles()!!.forEach { assertTrue(it.name, Regex("""[a-z][a-z0-9_]*\.wav""").matches(it.name)) }
    }

    @Test fun soundTableCoversEveryBundledSound() {
        val src = File("src/main/kotlin/com/macha/android/sound/SoundPlayer.kt").readText()
        val mapped = Regex("\"(\\w+)\" to R\\.raw\\.(\\w+)").findAll(src).map { assertEquals(it.groupValues[1], it.groupValues[2]); it.groupValues[1] }.toSet()
        assertEquals(raw.listFiles()!!.map { it.nameWithoutExtension }.toSet(), mapped)
    }

    @Test fun notificationIconIsMonochromeVector() {
        assertTrue(File(res, "drawable/ic_stat_macha.xml").readText().contains("#FFFFFFFF"))
    }

    @Test fun launcherIconIsPresent() {
        assertTrue(File(res, "mipmap-xxxhdpi/ic_launcher.png").length() > 1000)
    }
}
