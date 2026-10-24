package com.macha.android

import org.junit.Assert.assertEquals
import org.junit.Test

class AttributionTest {
    @Test fun nameAndRepoMatchPermission() {
        assertEquals("Macha for Android", Attribution.APP_NAME)
        assertEquals("https://github.com/dev-praj/Macha_by_Prajwal.git", Attribution.REPO_URL)
    }
}
