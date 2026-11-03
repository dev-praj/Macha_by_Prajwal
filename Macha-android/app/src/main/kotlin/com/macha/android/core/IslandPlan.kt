package com.macha.android.core

import com.macha.android.link.ApprovalRequest
import com.macha.android.link.SessionInfo
import com.macha.android.macha.BotState

/**
 * Which session the island is about, and whether it may show at all. Pure: the same inputs always
 * give the same answer, so every rule is unit-tested.
 */
object IslandPlan {
    /** The switch, the system permission and Macha not being on screen. */
    fun allowed(overlayOn: Boolean, permitted: Boolean, appInForeground: Boolean) =
        OverlayPolicy.shouldShow(overlayOn, permitted, appInForeground)

    /**
     * Items that need attention: approvals and questions. Ordinary work stays in the app and must
     * not leave a persistent overlay over other tasks. [approvals] come first; [name] gives the agent's name.
     */
    fun active(
        sessions: List<SessionInfo>,
        approvals: List<ApprovalRequest>,
        allowed: Boolean,
        name: (String) -> String,
    ): IslandSpec? {
        if (!allowed) return null
        approvals.firstOrNull()?.let {
            return IslandSpec(IslandSpec.Kind.APPROVAL, it.pillId, name(it.pillId), BotState.APPROVAL, ToolLabels.action(it.tool), it.fingerprint, ApprovalSheetPlan.canAllowHere(it))
        }
        val question = sessions.firstOrNull { it.state == BotState.QUESTION }
        if (question != null) return spec(IslandSpec.Kind.QUESTION, question, name)
        return null
    }

    /**
     * A finished, failed or rate-limited agent: only on a change into that state (never the first
     * picture after connecting), only if the user wants those notices, and not during quiet hours.
     * The same rule as [StatusPolicy.announce]: the pill part.
     */
    fun flash(
        previous: BotState?,
        session: SessionInfo,
        settings: UserSettings,
        minuteOfDay: Int,
        allowed: Boolean,
        name: (String) -> String,
    ): IslandSpec? {
        if (!allowed) return null
        val kind = when (session.state) {
            BotState.FINISHED -> IslandSpec.Kind.FINISHED
            BotState.ERROR -> IslandSpec.Kind.ERROR
            BotState.RATELIMIT -> IslandSpec.Kind.RATELIMIT
            else -> return null
        }
        val a = StatusPolicy.announce(previous, session.state, settings, minuteOfDay, appInForeground = false, pillId = session.pillId)
        if (!a.pill) return null
        return spec(kind, session, name)
    }

    private fun spec(kind: IslandSpec.Kind, s: SessionInfo, name: (String) -> String) =
        IslandSpec(kind, s.pillId, name(s.pillId), s.state, ToolLabels.label(s.statusText))
}
