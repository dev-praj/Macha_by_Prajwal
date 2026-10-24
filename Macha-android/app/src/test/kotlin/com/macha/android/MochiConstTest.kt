package com.macha.android

import com.macha.android.mochi.BotState
import com.macha.android.mochi.EyeShape
import com.macha.android.mochi.MochiConst
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assume.assumeTrue
import org.junit.Test

/** Checks the Android Mochi state table against Macha's Windows engine. */
class MochiConstTest {
    private fun source(): String {
        val file = ReferenceFiles.file("windows/src/macha/engine.ts")
        assumeTrue("Macha desktop source is not present", file.exists())
        return file.readText()
    }

    @Test fun stateColoursMatchReference() {
        val src = source()
        val block = src.substringAfter("const C = {").substringBefore("};")
        val re = Regex("""(\w+):\s*\[([\d.]+),\s*([\d.]+),\s*([\d.]+)]""")
        val ref = re.findAll(block).associate { it.groupValues[1] to Triple(it.groupValues[2], it.groupValues[3], it.groupValues[4]) }
        assertEquals(11, ref.size)
        for (s in BotState.entries) {
            val (r, g, b) = ref.getValue(s.key)
            val c = MochiConst.STATES.getValue(s).color
            assertEquals("${s.key}.r", r.toDouble(), c.r, 1e-9)
            assertEquals("${s.key}.g", g.toDouble(), c.g, 1e-9)
            assertEquals("${s.key}.b", b.toDouble(), c.b, 1e-9)
        }
    }

    @Test fun stateTintsAndEyesMatchReference() {
        val src = source()
        val block = src.substringAfter("export const BOT_STATES").substringBefore("/** State")
        val re = Regex("""(\w+): \{ \.\.\.base, color: C\.\w+, tint: ([\d.]+), eye: "(\w+)"""")
        val rows = re.findAll(block).toList()
        assertEquals(11, rows.size)
        for (m in rows) {
            val state = BotState.entries.first { it.key == m.groupValues[1] }
            val cfg = MochiConst.STATES.getValue(state)
            assertEquals(state.key, m.groupValues[2].toDouble(), cfg.tint, 1e-9)
            assertEquals(state.key, EyeShape.valueOf(m.groupValues[3].uppercase()), cfg.eye)
        }
    }

    @Test fun stateSoundsMatchReferenceAndFilesExist() {
        val src = source()
        val block = src.substringAfter("export const STATE_SOUND").substringBefore("};")
        val ref = Regex("""(\w+): "(\w+)"""").findAll(block).associate { it.groupValues[1] to it.groupValues[2] }
        for ((state, sound) in MochiConst.STATE_SOUND) assertEquals(ref[state.key], sound)
        assertEquals(ref.size, MochiConst.STATE_SOUND.size)
        val desktopSounds = ReferenceFiles.file("windows/assets/sounds")
        if (desktopSounds.exists()) {
            for (sound in ref.values) assertNotNull(ReferenceFiles.file("windows/assets/sounds/$sound.wav").takeIf { it.exists() })
        }
    }
}
