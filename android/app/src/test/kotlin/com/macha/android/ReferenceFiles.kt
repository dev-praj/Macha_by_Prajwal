package com.macha.android

import java.io.File

/** Locates the Macha desktop sources (windows/src/...) for Android interoperability tests. */
object ReferenceFiles {
    private fun repoRoot(): File {
        val currentDir = System.getProperty("user.dir") ?: error("user.dir is not available")
        var dir = File(currentDir).absoluteFile
        while (true) {
            if (File(dir, "windows/src/core/state.ts").exists()) return dir
            dir = dir.parentFile ?: error("repo root with windows/src not found from $currentDir")
        }
    }

    fun read(path: String): String = File(repoRoot(), path).readText()
    fun file(path: String): File = File(repoRoot(), path)
}
