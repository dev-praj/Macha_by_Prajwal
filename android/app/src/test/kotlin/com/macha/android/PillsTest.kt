package com.macha.android

import com.macha.android.core.PillCategory
import com.macha.android.core.Pills
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** Every built-in Macha integration must be displayable by the Android client. */
class PillsTest {
    private data class Ref(val id: String, val name: String, val color: String)

    private fun reference(): List<Ref> {
        val src = ReferenceFiles.read("windows/src/core/state.ts")
        val block = src.substringAfter("export const INTEGRATION_AGENTS").substringBefore("];")
        return Regex("""task\("([^"]+)",\s*"([^"]+)",\s*"(#[0-9A-Fa-f]{6})"""")
            .findAll(block)
            .map { Ref(it.groupValues[1], it.groupValues[2], it.groupValues[3]) }
            .toList()
    }

    @Test fun everyBuiltInMachaIntegrationMatchesDesktopMetadata() {
        val expected = reference()
        assertEquals(8, expected.size)
        for (ref in expected) {
            val phone = Pills.byId(ref.id)
            assertTrue("${ref.id} is missing from the phone catalog", phone != null)
            assertEquals(ref.name, phone!!.name)
            assertEquals(ref.color.uppercase(), phone.colorHex.uppercase())
        }
    }

    @Test fun idsAreUniqueAndDefaultExists() {
        assertEquals(Pills.catalog.size, Pills.catalog.map { it.id }.toSet().size)
        assertEquals(PillCategory.WORKSPACE, Pills.byId(Pills.DEFAULT_MAIN_PILL)?.category)
    }
}
