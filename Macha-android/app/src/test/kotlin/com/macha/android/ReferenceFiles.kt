package com.macha.android

import java.io.File

/** Locates the Macha desktop sources (windows/src/...) for Android interoperability tests. */
object ReferenceFiles {
    private fun androidRoot(): File {
        val currentDir = System.getProperty("user.dir") ?: error("user.dir is not available")
        var dir = File(currentDir).absoluteFile
        while (true) {
            if (File(dir, "settings.gradle.kts").exists() && File(dir, "app/src/main").isDirectory) return dir
            dir = dir.parentFile ?: error("Android project root not found from $currentDir")
        }
    }

    private fun repoRoot(): File {
        val currentDir = System.getProperty("user.dir") ?: error("user.dir is not available")
        var dir = File(currentDir).absoluteFile
        while (true) {
            if (File(dir, "windows/src/core/state.ts").exists()) return dir
            dir = dir.parentFile ?: return androidRoot()
        }
    }

    fun file(path: String): File {
        val normalized = path.replace('\\', '/')
        return if (normalized.startsWith("android/")) {
            File(androidRoot(), normalized.removePrefix("android/"))
        } else {
            File(repoRoot(), path)
        }
    }

    fun read(path: String): String = file(path).readText()
}
