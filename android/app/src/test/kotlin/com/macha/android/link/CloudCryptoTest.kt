package com.macha.android.link

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class CloudCryptoTest {
    @Test fun x25519AndHkdfMatchAcrossBothSides() {
        val laptop = CloudCrypto.generateIdentity()
        val phone = CloudCrypto.generateIdentity()
        val laptopKey = CloudCrypto.derive(laptop, phone.publicEncoded(), "link-id")
        val phoneKey = CloudCrypto.derive(phone, laptop.publicEncoded(), "link-id")
        assertEquals(laptopKey.toList(), phoneKey.toList())
        assertNotEquals(laptopKey.toList(), CloudCrypto.derive(phone, laptop.publicEncoded(), "other-link").toList())
    }

    @Test fun aesGcmEnvelopeAuthenticatesAndRejectsReplay() {
        val sender = CloudCrypto.Codec(ByteArray(32) { it.toByte() })
        val receiver = CloudCrypto.Codec(ByteArray(32) { it.toByte() })
        val envelope = sender.seal("approval payload")
        assertEquals("approval payload", receiver.open(envelope))
        assertThrows(IllegalArgumentException::class.java) { receiver.open(envelope) }
        val modified = sender.seal("second").copy(ciphertext = sender.seal("third").ciphertext)
        assertThrows(Exception::class.java) { receiver.open(modified) }
    }
}
