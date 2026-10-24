package com.macha.android.link

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test

class ProtocolServicesTest {
    private fun cards(json: String) = (Wire.decodeServer("""{"type":"services","services":$json}""") as ServerMsg.Services).cards

    @Test fun aCardIsRead() {
        val c = cards("""[{"id":"integration_stripe","title":"Stripe","headline":"12.50 EUR","reason":"Payments","items":[{"label":"Payment","detail":"+9.00 · 2m"}]}]""")
        assertEquals(ServiceCard("integration_stripe", "Stripe", "12.50 EUR", "Payments", listOf(ServiceLine("Payment", "+9.00 · 2m"))), c[0])
    }

    @Test fun anEmptyListTakesTheCardsAway() {
        assertEquals(emptyList<ServiceCard>(), cards("[]"))
    }

    @Test fun onlyTheKnownServicesAndOnceEach() {
        val c = cards("""[{"id":"integration_evil","title":"x","headline":"y"},{"id":"integration_stripe","title":"S","headline":"1"},{"id":"integration_stripe","title":"again","headline":"2"},5,"junk"]""")
        assertEquals(listOf("integration_stripe"), c.map { it.id })
        assertEquals("S", c[0].title)
    }

    @Test fun textsAreOneShortLineAndThereAreAtMostThreeLines() {
        val long = "x".repeat(300)
        val c = cards(
            """[{"id":"integration_github","title":"G","headline":"$long\n\u0007","reason":"  ","items":[{"label":"a","detail":"1"},{"label":"b","detail":"2"},{"label":"","detail":"empty"},{"label":"c","detail":"3"},{"label":"d","detail":"4"}]}]""",
        )[0]
        assertEquals(Protocol.MAX_SERVICE_TEXT, c.headline.length)
        assertFalse(c.headline.any { it.isISOControl() })
        assertEquals(null, c.reason)
        assertEquals(listOf("a", "b", "c"), c.items.map { it.label })
    }

    @Test fun aCardMissingItsFieldsStillReads() {
        val c = cards("""[{"id":"integration_calcom"}]""")[0]
        assertEquals("", c.headline)
        assertEquals(emptyList<ServiceLine>(), c.items)
    }

    @Test fun theHelloAsksForServices() {
        assertTrue(Protocol.CAP_SERVICES in Protocol.CAPABILITIES)
    }

    @Test fun theKnownIdsMatchTheDesktopPhoneLinkAllowlist() {
        val file = com.macha.android.ReferenceFiles.file("windows/src-tauri/src/phone_link/hub.rs")
        assumeTrue("Macha desktop phone-link source is not present", file.exists())
        val source = file.readText()
        val block = Regex("""pub const SERVICE_IDS: \[&str; \d+] = \[(.*?)]""", RegexOption.DOT_MATCHES_ALL)
            .find(source)!!.groupValues[1]
        val ids = Regex(""""(integration_[^"]+)"""").findAll(block).map { it.groupValues[1] }.toList()
        assertEquals(5, ids.size)
        for (id in ids) {
            assertTrue(id, source.contains("\"$id\""))
            assertEquals(id, 1, cards("""[{"id":"$id","title":"t","headline":"h"}]""").size)
        }
    }
}
