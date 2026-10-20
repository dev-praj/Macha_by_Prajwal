#!/usr/bin/env python3
"""
Vision Mode - v1.0.0
- Hand detection runs entirely in Python (OpenCV + MediaPipe)
- Always-on-top floating camera window (tkinter) stays visible even when browser is minimized
- WebSocket server sends mouse commands to pyautogui
- Browser dashboard for settings/status (optional)

Usage: python app.py
"""

import argparse
import asyncio
import ctypes
import json
import os
import sys
import threading
import webbrowser
import time
import queue
import math

import cv2
import mediapipe as mp
import numpy as np
from mediapipe.tasks import python as mp_tasks
from mediapipe.tasks.python import vision as mp_vision
from mediapipe.tasks.python.vision import (
    HandLandmarker, HandLandmarkerOptions, RunningMode
)
import pyautogui
from aiohttp import web

if getattr(sys, "frozen", False):
    try:
        ctypes.windll.shcore.SetProcessDpiAwareness(2)
    except (AttributeError, OSError):
        pass

# ── PyAutoGUI config ──────────────────────────────────────────────────────────
pyautogui.FAILSAFE = True
pyautogui.PAUSE = 0

screen_width, screen_height = pyautogui.size()
print(f"Screen size: {screen_width}x{screen_height}")
CURSOR_MARGIN = 8

# ── Model path ────────────────────────────────────────────────────────────────
APP_DATA_DIR = os.path.join(
    os.environ.get("LOCALAPPDATA") or os.path.expanduser("~"),
    "Macha",
    "gesture_mouse",
)
os.makedirs(APP_DATA_DIR, exist_ok=True)


def resource_path(relative):
    base_path = getattr(sys, "_MEIPASS", os.path.dirname(os.path.abspath(__file__)))
    return os.path.join(base_path, relative)


if getattr(sys, "frozen", False):
    MODEL_PATH = resource_path("hand_landmarker.task")
    if not os.path.isfile(MODEL_PATH):
        raise FileNotFoundError(f"Bundled hand model was not found: {MODEL_PATH}")
else:
    MODEL_PATH = os.path.join(APP_DATA_DIR, "hand_landmarker.task")
    if not os.path.exists(MODEL_PATH):
        print("Downloading hand_landmarker.task model...")
        import urllib.request
        urllib.request.urlretrieve(
            "https://storage.googleapis.com/mediapipe-models/hand_landmarker/hand_landmarker/float16/1/hand_landmarker.task",
            MODEL_PATH
        )
        print("Model downloaded.")

# ── One Euro Filter (jitter removal) ─────────────────────────────────────────
class LowPassFilter:
    """Simple exponential low-pass filter."""
    def __init__(self, alpha=1.0):
        self.y = None
        self.alpha = alpha

    def __call__(self, value, alpha=None):
        if alpha is not None:
            self.alpha = alpha
        if self.y is None:
            self.y = value
        else:
            self.y = self.alpha * value + (1 - self.alpha) * self.y
        return self.y

    def reset(self):
        self.y = None


class OneEuroFilter:
    """One Euro Filter — speed-adaptive jitter reduction.
    Low min_cutoff = smoother when still.  High beta = more responsive to fast moves."""
    def __init__(self, freq=60.0, min_cutoff=1.0, beta=0.007, d_cutoff=1.0):
        self.freq = freq
        self.min_cutoff = min_cutoff
        self.beta = beta
        self.d_cutoff = d_cutoff
        self.x_filter = LowPassFilter()
        self.dx_filter = LowPassFilter()
        self.last_time = None

    def _alpha(self, cutoff):
        te = 1.0 / self.freq
        tau = 1.0 / (2 * math.pi * cutoff)
        return 1.0 / (1.0 + tau / te)

    def __call__(self, x, t=None):
        if self.last_time is not None and t is not None:
            self.freq = 1.0 / max(t - self.last_time, 1e-9)
        self.last_time = t

        prev = self.x_filter.y
        dx = 0.0 if prev is None else (x - prev) * self.freq
        edx = self.dx_filter(dx, self._alpha(self.d_cutoff))
        cutoff = self.min_cutoff + self.beta * abs(edx)
        return self.x_filter(x, self._alpha(cutoff))

    def reset(self):
        self.x_filter.reset()
        self.dx_filter.reset()
        self.last_time = None


# ── Shared state (thread-safe via simple primitives) ──────────────────────────
class AppState:
    def __init__(self):
        self.running = True
        self.lock = threading.Lock()
        self.pointer_lock = threading.Lock()

        # Settings (can be updated from browser)
        self.pinch_threshold = 0.05
        self.smoothing = 0.3          # controls One Euro Filter min_cutoff
        self.sensitivity = 1.0        # 0.5 = big hand moves, 2.0 = tiny hand moves

        # Derived hand range (recomputed when sensitivity changes)
        self._base_center = 0.5
        self._base_half = 0.2         # default range is 0.3–0.7
        self._update_hand_range()

        # Target position from detection thread (raw, before filtering)
        self.target_x = screen_width / 2.0
        self.target_y = screen_height / 2.0

        # Actual cursor position (updated by cursor thread at 60fps)
        self.curr_x = screen_width / 2.0
        self.curr_y = screen_height / 2.0

        self.last_left = False
        self.last_right = False
        self.last_scroll = False
        self.mouse_button_down = False
        self.smooth_scroll_y = None
        self.cursor_locked = False
        self.locked_x = screen_width / 2.0
        self.locked_y = screen_height / 2.0
        self.palm_gesture_active = False
        self.palm_gesture_since = None
        self.palm_last_seen = None
        self.palm_toggle_armed = True
        self.palm_hold_progress = 0.0
        self.palm_completion_time = None
        self.palm_completion_locked = False
        self.last_locked_scroll_point = None
        self.locked_scroll_origin_point = None
        self.locked_scroll_motion_started = False
        self.locked_scroll_remainder_x = 0.0
        self.locked_scroll_remainder_y = 0.0
        self.locked_scroll_pose_active = False
        self.locked_scroll_pose_since = None
        self.locked_scroll_pose_last_seen = None
        self.last_rotate_angle = None
        self.rotate_pose_active = False
        self.rotate_pose_since = None
        self.rotate_pose_last_seen = None
        self.rotate_drag_offset = 0.0
        self.rotate_angle_remainder = 0.0
        self.rotate_filtered_angle = None
        self.rotate_drag_active = False
        self.rotate_drag_position_x = None
        self.hand_mouse_active = True
        self.fist_gesture_since = None
        self.fist_last_seen = None
        self.fist_release_since = None
        self.fist_toggle_armed = True
        self.fist_hold_progress = 0.0
        self.fist_cooldown_until = 0.0
        self.fist_output_blocked = False

        # Scroll constants
        self.SCROLL_SMOOTHING = 0.3
        self.SCROLL_SENSITIVITY = 1800
        self.SCROLL_DEAD_ZONE = 5
        self.LOCKED_SCROLL_SENSITIVITY = 100

        # Status for dashboard
        self.camera_ready = False
        self.hand_detected = False
        self.gesture_left = False
        self.gesture_right = False
        self.gesture_scroll = False
        self.gesture_locked_scroll = False
        self.gesture_rotate = False
        self.fps = 0
        self.gesture_count = 0

        # Frame queue for tkinter window (latest frame only)
        self.frame_queue = queue.Queue(maxsize=2)

        # WebSocket clients for broadcasting status
        self.ws_clients = set()

    def _update_hand_range(self):
        """Recompute hand_range_min/max from sensitivity."""
        half = self._base_half / max(self.sensitivity, 0.1)
        self.hand_range_min = max(0.0, self._base_center - half)
        self.hand_range_max = min(1.0, self._base_center + half)

state = AppState()
PALM_HOLD_SECONDS = 5.0
PALM_TRACKING_GRACE = 0.18
GESTURE_TRACKING_GRACE = 0.12
GESTURE_POSE_CONFIRMATION = 0.08
ROTATE_DRAG_SENSITIVITY = 240
FIST_HOLD_MS = 1000
FIST_TRACKING_GRACE_MS = 200
FIST_RELEASE_COOLDOWN_MS = 500
FIST_MIN_HAND_SIZE = 1e-6
FIST_FINGER_TIP_MCP_MAX_RATIO = 0.65
FIST_THUMB_BASE_MAX_RATIO = 0.45
FIST_THUMB_PALM_MAX_RATIO = 0.8

# ── MediaPipe hand connections (for drawing) ──────────────────────────────────
# New Tasks API: connections are frozensets of (start_idx, end_idx) tuples
HAND_CONNECTIONS = [
    (0,1),(1,2),(2,3),(3,4),       # thumb
    (0,5),(5,6),(6,7),(7,8),       # index
    (0,9),(9,10),(10,11),(11,12),  # middle
    (0,13),(13,14),(14,15),(15,16),# ring
    (0,17),(17,18),(18,19),(19,20),# pinky
    (5,9),(9,13),(13,17),          # palm
]

# ── Colour palette ────────────────────────────────────────────────────────────
ACCENT       = (231, 92, 108)   # BGR: #6c5ce7 → purple
ACCENT_LIGHT = (254, 155, 162)  # BGR: #a29bfe
LEFT_COLOR   = (110, 203, 253)  # BGR: #fdcb6e → yellow
RIGHT_COLOR  = (255, 185, 116)  # BGR: #74b9ff → blue
SCROLL_COLOR = (254, 155, 162)  # BGR: #a29bfe → purple
GREEN        = (0, 255, 0)
RED          = (0, 0, 255)
WHITE        = (255, 255, 255)
DARK_BG      = (19, 15, 15)     # BGR: #0f0f13


def dist(p1, p2):
    return math.sqrt((p1.x - p2.x) ** 2 + (p1.y - p2.y) ** 2)


def remap(val, in_min, in_max):
    val = max(in_min, min(val, in_max))
    return (val - in_min) / (in_max - in_min)


def wrap_angle(angle):
    return (angle + math.pi) % (2 * math.pi) - math.pi


def scroll_steps(delta, remainder):
    total = delta + remainder
    steps = math.trunc(total)
    return steps, total - steps


def joint_angle(first, joint, last):
    ax, ay = first.x - joint.x, first.y - joint.y
    bx, by = last.x - joint.x, last.y - joint.y
    lengths = math.hypot(ax, ay) * math.hypot(bx, by)
    if lengths < 1e-9:
        return 0.0
    cosine = max(-1.0, min(1.0, (ax * bx + ay * by) / lengths))
    return math.degrees(math.acos(cosine))


def finger_extended(landmarks, mcp_index, pip_index, dip_index):
    straight_angle = joint_angle(
        landmarks[mcp_index], landmarks[pip_index], landmarks[dip_index]
    )
    return straight_angle >= 140.0


def hand_finger_states(landmarks):
    palm_width = max(dist(landmarks[5], landmarks[17]), 0.001)
    thumb_extended = (
        joint_angle(landmarks[1], landmarks[2], landmarks[3]) >= 135.0
        and dist(landmarks[4], landmarks[5]) > palm_width * 0.35
    )
    return (
        thumb_extended,
        finger_extended(landmarks, 5, 6, 7),
        finger_extended(landmarks, 9, 10, 11),
        finger_extended(landmarks, 13, 14, 15),
        finger_extended(landmarks, 17, 18, 19),
        palm_width,
    )


def finger_fully_extended(landmarks, mcp_index, pip_index, dip_index, tip_index):
    return (
        joint_angle(
            landmarks[mcp_index], landmarks[pip_index], landmarks[dip_index]
        ) >= 160.0
        and joint_angle(
            landmarks[pip_index], landmarks[dip_index], landmarks[tip_index]
        ) >= 150.0
    )


def is_open_palm(fingers, landmarks):
    palm_width = max(dist(landmarks[5], landmarks[17]), 0.001)
    thumb_fully_open = (
        fingers[0]
        and joint_angle(landmarks[2], landmarks[3], landmarks[4]) >= 155.0
        and dist(landmarks[4], landmarks[5]) > palm_width * 0.45
    )
    fingers_fully_open = all(
        finger_fully_extended(landmarks, mcp, pip, dip, tip)
        for mcp, pip, dip, tip in (
            (5, 6, 7, 8),
            (9, 10, 11, 12),
            (13, 14, 15, 16),
            (17, 18, 19, 20),
        )
    )
    return thumb_fully_open and fingers_fully_open


def update_palm_hold(palm_open, now):
    with state.lock:
        hold_completed = False
        if palm_open:
            state.palm_last_seen = now
            if state.palm_toggle_armed:
                if state.palm_gesture_since is None:
                    state.palm_gesture_since = now
                elapsed = now - state.palm_gesture_since
                state.palm_hold_progress = min(elapsed / PALM_HOLD_SECONDS, 1.0)
                if elapsed >= PALM_HOLD_SECONDS:
                    state.palm_toggle_armed = False
                    state.palm_hold_progress = 1.0
                    state.palm_completion_time = now
                    hold_completed = True
            else:
                state.palm_hold_progress = 1.0
            palm_effective = True
        else:
            state.palm_gesture_since = None
            state.palm_last_seen = None
            state.palm_hold_progress = 0.0
            state.palm_toggle_armed = True
            palm_effective = False
        state.palm_gesture_active = palm_effective
        return hold_completed, palm_effective, state.cursor_locked


def is_closed_fist(landmarks):
    """Recognize a scale-normalized fist from palm-relative landmark distances."""
    hand_size = dist(landmarks[0], landmarks[9])
    if hand_size <= FIST_MIN_HAND_SIZE:
        return False

    wrist = landmarks[0]
    for tip, pip, mcp in (
        (8, 6, 5),
        (12, 10, 9),
        (16, 14, 13),
        (20, 18, 17),
    ):
        if (
            dist(landmarks[tip], wrist) >= dist(landmarks[pip], wrist)
            or dist(landmarks[tip], landmarks[mcp])
            >= hand_size * FIST_FINGER_TIP_MCP_MAX_RATIO
        ):
            return False

    thumb_to_index_mcp = dist(landmarks[4], landmarks[5])
    thumb_to_middle_mcp = dist(landmarks[4], landmarks[9])
    thumb_tucked = min(thumb_to_index_mcp, thumb_to_middle_mcp) < (
        hand_size * FIST_THUMB_BASE_MAX_RATIO
    )
    thumb_not_extended = thumb_to_middle_mcp < hand_size * FIST_THUMB_PALM_MAX_RATIO
    return thumb_tucked and thumb_not_extended


def update_fist_hold(fist_detected, hand_present, now):
    """Update the fist timer, pause state, and output-suppression state."""
    with state.pointer_lock, state.lock:
        was_blocked = state.fist_output_blocked
        was_active = state.hand_mouse_active
        toggled = False

        if fist_detected and hand_present:
            state.fist_last_seen = now
            state.fist_release_since = None
            state.fist_output_blocked = True
            if state.fist_toggle_armed:
                if (
                    state.fist_gesture_since is None
                    and now >= state.fist_cooldown_until
                ):
                    state.fist_gesture_since = now
                if state.fist_gesture_since is not None:
                    elapsed = now - state.fist_gesture_since
                    state.fist_hold_progress = min(
                        elapsed / (FIST_HOLD_MS / 1000.0), 1.0
                    )
                    if elapsed >= FIST_HOLD_MS / 1000.0:
                        state.hand_mouse_active = not state.hand_mouse_active
                        state.fist_toggle_armed = False
                        state.fist_hold_progress = 1.0
                        toggled = True
            else:
                state.fist_hold_progress = 1.0
        elif not hand_present:
            released = (
                state.fist_last_seen is not None
                or state.fist_gesture_since is not None
                or not state.fist_toggle_armed
            )
            state.fist_gesture_since = None
            state.fist_last_seen = None
            state.fist_release_since = None
            state.fist_hold_progress = 0.0
            state.fist_output_blocked = False
            state.fist_toggle_armed = True
            if released:
                state.fist_cooldown_until = (
                    now + FIST_RELEASE_COOLDOWN_MS / 1000.0
                )
        elif (
            state.fist_last_seen is not None
            and now - state.fist_last_seen <= FIST_TRACKING_GRACE_MS / 1000.0
        ):
            state.fist_output_blocked = True
            if state.fist_release_since is None:
                state.fist_release_since = now
            if state.fist_gesture_since is not None and state.fist_toggle_armed:
                state.fist_hold_progress = min(
                    (now - state.fist_gesture_since) / (FIST_HOLD_MS / 1000.0),
                    0.999,
                )
        else:
            released = (
                state.fist_last_seen is not None
                or state.fist_gesture_since is not None
                or not state.fist_toggle_armed
            )
            state.fist_gesture_since = None
            cooldown_start = state.fist_release_since or now
            state.fist_last_seen = None
            state.fist_release_since = None
            state.fist_hold_progress = 0.0
            state.fist_output_blocked = False
            state.fist_toggle_armed = True
            if released:
                state.fist_cooldown_until = (
                    cooldown_start + FIST_RELEASE_COOLDOWN_MS / 1000.0
                )

        release_mouse = (
            state.fist_output_blocked and not was_blocked
        ) or (was_active and not state.hand_mouse_active)
        return (
            toggled,
            state.hand_mouse_active,
            state.fist_output_blocked,
            state.fist_hold_progress,
            release_mouse,
        )


def release_held_mouse_button():
    """Release an active click or drag before fist suppression takes effect."""
    with state.pointer_lock:
        with state.lock:
            release = state.mouse_button_down
        if release:
            failsafe = pyautogui.FAILSAFE
            pyautogui.FAILSAFE = False
            try:
                pyautogui.mouseUp()
            finally:
                pyautogui.FAILSAFE = failsafe
        with state.lock:
            state.mouse_button_down = False
            state.rotate_drag_active = False
            state.rotate_drag_position_x = None
            state.rotate_pose_active = False
            state.rotate_pose_since = None
            state.rotate_pose_last_seen = None
            state.last_locked_scroll_point = None
            state.locked_scroll_origin_point = None
            state.locked_scroll_motion_started = False
            state.locked_scroll_pose_active = False
            state.locked_scroll_pose_since = None
            state.locked_scroll_pose_last_seen = None


def mouse_output_enabled():
    with state.lock:
        return state.hand_mouse_active and not state.fist_output_blocked


def is_locked_scroll_pose(fingers, landmarks, palm_width):
    thumb, index, middle, ring, pinky = fingers[:5]
    fingertips_together = dist(landmarks[8], landmarks[12]) < palm_width * 0.7
    return not thumb and index and middle and not ring and not pinky and fingertips_together


def is_rotate_pose(fingers, landmarks, palm_width):
    thumb, index, middle, ring, pinky = fingers[:5]
    thumb_index_pinch = dist(landmarks[4], landmarks[8]) < palm_width * 0.45
    return (
        ((thumb and index) or thumb_index_pinch)
        and not middle
        and not ring
        and not pinky
    )


def rotation_drag_delta(angle_delta):
    return angle_delta * ROTATE_DRAG_SENSITIVITY


def clamp_cursor(value, screen_length):
    if screen_length <= 1:
        return 0
    margin = min(CURSOR_MARGIN, (screen_length - 1) // 2)
    return max(margin, min(value, screen_length - margin - 1))


def draw_rounded_rect(img, pt1, pt2, color, radius=10, thickness=-1, alpha=0.6):
    """Draw a semi-transparent rounded rectangle."""
    overlay = img.copy()
    x1, y1 = pt1
    x2, y2 = pt2
    cv2.rectangle(overlay, (x1 + radius, y1), (x2 - radius, y2), color, thickness)
    cv2.rectangle(overlay, (x1, y1 + radius), (x2, y2 - radius), color, thickness)
    cv2.circle(overlay, (x1 + radius, y1 + radius), radius, color, thickness)
    cv2.circle(overlay, (x2 - radius, y1 + radius), radius, color, thickness)
    cv2.circle(overlay, (x1 + radius, y2 - radius), radius, color, thickness)
    cv2.circle(overlay, (x2 - radius, y2 - radius), radius, color, thickness)
    cv2.addWeighted(overlay, alpha, img, 1 - alpha, 0, img)


def draw_overlay(
    frame, hand_detected, is_left, is_right, is_scroll, is_locked_scroll,
    is_rotate, cursor_locked, palm_hold_progress, palm_completion_locked,
    palm_feedback_age, landmarks, fps, hand_mouse_active, fist_hold_progress,
):
    """Draw HUD overlay on the camera frame."""
    h, w = frame.shape[:2]

    # Semi-transparent top bar
    overlay = frame.copy()
    cv2.rectangle(overlay, (0, 0), (w, 36), (20, 20, 30), -1)
    cv2.addWeighted(overlay, 0.75, frame, 0.25, 0, frame)

    # Title
    cv2.putText(frame, "Vision Mode", (10, 24),
                cv2.FONT_HERSHEY_SIMPLEX, 0.6, (162, 155, 254), 1, cv2.LINE_AA)

    if palm_feedback_age is not None and palm_feedback_age < 1.8:
        mode_text = "CURSOR LOCKED" if palm_completion_locked else "CURSOR UNLOCKED"
        mode_color = (0, 220, 180) if palm_completion_locked else (0, 210, 255)
    elif cursor_locked:
        mode_text = "CURSOR LOCKED"
        mode_color = (0, 220, 180)
    elif palm_hold_progress > 0:
        mode_text = f"PALM HOLD {int(palm_hold_progress * 100)}%"
        mode_color = (0, 210, 255)
    else:
        mode_text = "OPEN PALM 5s TO LOCK"
        mode_color = (136, 136, 168)
    cv2.putText(frame, mode_text, (145, 24),
                cv2.FONT_HERSHEY_SIMPLEX, 0.42, mode_color, 1, cv2.LINE_AA)

    if hand_detected and (palm_hold_progress > 0 or (
        palm_feedback_age is not None and palm_feedback_age < 1.8
    )):
        center = (
            int(landmarks[9].x * w),
            int(landmarks[9].y * h),
        )
        palm_width = dist(landmarks[5], landmarks[17]) * w
        radius = max(26, int(palm_width * 0.8))
        pulse = (
            int(5 * (0.5 + 0.5 * math.sin(palm_feedback_age * 9)))
            if palm_feedback_age is not None and palm_feedback_age < 1.8
            else 0
        )
        radius += pulse
        cv2.circle(frame, center, radius, (80, 80, 105), 3, cv2.LINE_AA)
        progress = (
            1.0 if palm_feedback_age is not None and palm_feedback_age < 1.8
            else palm_hold_progress
        )
        progress_color = (
            (0, 220, 180) if palm_feedback_age is not None and palm_feedback_age < 1.8
            else (0, 210, 255)
        )
        cv2.ellipse(
            frame, center, (radius, radius), 0, -90,
            -90 + int(360 * progress), progress_color, 5, cv2.LINE_AA,
        )
        cv2.putText(
            frame,
            "DONE" if progress >= 1.0 else f"{int(progress * 100)}%",
            (center[0] - 20, center[1] + 6),
            cv2.FONT_HERSHEY_SIMPLEX, 0.48, WHITE, 1, cv2.LINE_AA,
        )

    # FPS
    fps_text = f"FPS: {fps}"
    cv2.putText(frame, fps_text, (w - 80, 24),
                cv2.FONT_HERSHEY_SIMPLEX, 0.5, (136, 136, 168), 1, cv2.LINE_AA)

    # Keep the hand-mouse state and fist hold visible without changing its controls.
    mouse_status = "ACTIVE" if hand_mouse_active else "PAUSED"
    mouse_status_color = (0, 220, 180) if hand_mouse_active else (0, 120, 255)
    cv2.putText(
        frame, mouse_status, (w - 94, 48),
        cv2.FONT_HERSHEY_SIMPLEX, 0.4, mouse_status_color, 1, cv2.LINE_AA,
    )
    progress_left = w - 94
    progress_top = 54
    progress_width = 82
    cv2.rectangle(
        frame, (progress_left, progress_top),
        (progress_left + progress_width, progress_top + 5), (60, 60, 80), -1,
    )
    if fist_hold_progress > 0:
        cv2.rectangle(
            frame, (progress_left, progress_top),
            (
                progress_left + int(progress_width * fist_hold_progress),
                progress_top + 5,
            ),
            (0, 210, 255), -1,
        )

    # Hand status badge
    if hand_detected:
        badge_color = (201, 206, 0)   # BGR for teal
        badge_text = "HAND DETECTED"
    else:
        badge_color = (100, 100, 120)
        badge_text = "NO HAND"

    overlay2 = frame.copy()
    cv2.rectangle(overlay2, (8, h - 30), (8 + len(badge_text) * 9 + 8, h - 8),
                  badge_color, -1)
    cv2.addWeighted(overlay2, 0.35, frame, 0.65, 0, frame)
    cv2.putText(frame, badge_text, (12, h - 13),
                cv2.FONT_HERSHEY_SIMPLEX, 0.45,
                (0, 230, 220) if hand_detected else (100, 100, 120),
                1, cv2.LINE_AA)
    # Gesture indicators (bottom right)
    gestures = [
        ("L-CLICK", is_left,   LEFT_COLOR),
        ("R-CLICK", is_right,  RIGHT_COLOR),
        ("SCROLL",  is_scroll, SCROLL_COLOR),
        ("LOCK SCROLL", is_locked_scroll, SCROLL_COLOR),
        ("ROTATE", is_rotate, LEFT_COLOR),
    ]
    by = h - 30
    for i, (label, active, color) in enumerate(gestures):
        y = by - i * 18
        dot_color = color if active else (60, 60, 80)
        cv2.circle(frame, (w - 104, y + 5), 4, dot_color, -1)
        cv2.putText(frame, label, (w - 96, y + 9),
                    cv2.FONT_HERSHEY_SIMPLEX, 0.32,
                    color if active else (80, 80, 100),
                    1, cv2.LINE_AA)


def draw_hand_skeleton(frame, landmarks):
    """Draw hand skeleton using manual connections (Tasks API compatible)."""
    h, w = frame.shape[:2]
    pts = [(int(lm.x * w), int(lm.y * h)) for lm in landmarks]
    for (a, b) in HAND_CONNECTIONS:
        cv2.line(frame, pts[a], pts[b], ACCENT, 2, cv2.LINE_AA)
    for pt in pts:
        cv2.circle(frame, pt, 4, ACCENT_LIGHT, -1, cv2.LINE_AA)
        cv2.circle(frame, pt, 4, ACCENT, 1, cv2.LINE_AA)


# ── Hand detection thread ─────────────────────────────────────────────────────
def hand_detection_thread():
    cap = None
    try:
        cap = cv2.VideoCapture(0)
        _hand_detection_loop(cap)
    except Exception as error:
        with state.lock:
            state.camera_ready = False
            state.running = False
        print(f"ERROR: Hand detection workflow stopped: {error}")
    finally:
        try:
            if cap is not None:
                cap.release()
        except Exception as error:
            with state.lock:
                state.running = False
            print(f"ERROR: Could not release camera: {error}")
        finally:
            with state.lock:
                state.camera_ready = False


def _hand_detection_loop(cap):
    if not cap.isOpened():
        raise RuntimeError("Could not open camera.")

    cap.set(cv2.CAP_PROP_FRAME_WIDTH, 640)
    cap.set(cv2.CAP_PROP_FRAME_HEIGHT, 480)
    cap.set(cv2.CAP_PROP_FPS, 60)
    cap.set(cv2.CAP_PROP_BUFFERSIZE, 1)  # minimize capture buffer lag

    frame_count = 0
    fail_count = 0
    fps_timer = time.time()
    fps = 0

    opts = HandLandmarkerOptions(
        base_options=mp_tasks.BaseOptions(model_asset_path=MODEL_PATH),
        running_mode=RunningMode.VIDEO,   # VIDEO mode uses tracking → much faster
        num_hands=1,
        min_hand_detection_confidence=0.5,
        min_hand_presence_confidence=0.5,
        min_tracking_confidence=0.5,
    )

    with HandLandmarker.create_from_options(opts) as detector:
        while state.running:
            ret, frame = cap.read()
            if not ret:
                fail_count += 1
                with state.lock:
                    state.camera_ready = False
                if fail_count > 60:
                    print("ERROR: Camera disconnected or unavailable!")
                    state.running = False
                    break
                continue
            fail_count = 0

            frame_count += 1
            now = time.time()
            palm_now = time.monotonic()
            elapsed = now - fps_timer
            if elapsed >= 1.0:
                fps = int(frame_count / elapsed)
                frame_count = 0
                fps_timer = now
                with state.lock:
                    state.fps = fps

            # Flip for mirror view
            frame = cv2.flip(frame, 1)
            rgb = cv2.cvtColor(frame, cv2.COLOR_BGR2RGB)
            timestamp_ms = time.monotonic_ns() // 1_000_000
            mp_img = mp.Image(image_format=mp.ImageFormat.SRGB, data=rgb)
            result = detector.detect_for_video(mp_img, timestamp_ms)
            with state.lock:
                state.camera_ready = True

            hand_detected = False
            is_left = is_right = is_scroll = False
            is_locked_scroll = is_rotate = False
            palm_hold_progress = 0.0
            cursor_locked = False
            palm_completion_locked = False
            palm_feedback_age = None
            fist_hold_progress = 0.0
            with state.lock:
                hand_mouse_active = state.hand_mouse_active
                fist_output_blocked = state.fist_output_blocked
            landmarks = None
            if result.hand_landmarks:
                hand_detected = True
                landmarks = result.hand_landmarks[0]  # list of NormalizedLandmark
                fist_detected = is_closed_fist(landmarks)
                (
                    _,
                    hand_mouse_active,
                    fist_output_blocked,
                    fist_hold_progress,
                    release_mouse,
                ) = update_fist_hold(fist_detected, True, palm_now)
                if release_mouse:
                    try:
                        release_held_mouse_button()
                    except pyautogui.FailSafeException:
                        with state.lock:
                            state.running = False
                        print(
                            "PyAutoGUI emergency fail-safe triggered while pausing hand control."
                        )
                        break

                with state.lock:
                    thresh = state.pinch_threshold
                    rmin = state.hand_range_min
                    rmax = state.hand_range_max

                fingers = hand_finger_states(landmarks)
                palm_open = is_open_palm(fingers, landmarks)
                hold_completed, palm_effective, was_cursor_locked = update_palm_hold(
                    palm_open, palm_now
                )

                if hold_completed:
                    with state.pointer_lock:
                        cursor_position = None
                        try:
                            if not was_cursor_locked:
                                cursor_position = pyautogui.position()
                        except pyautogui.FailSafeException:
                            with state.lock:
                                state.running = False
                            print(
                                "PyAutoGUI emergency fail-safe triggered at a screen corner; "
                                "gesture control stopped."
                            )
                            break
                        with state.lock:
                            state.cursor_locked = not was_cursor_locked
                            state.palm_completion_locked = state.cursor_locked
                            if cursor_position is not None:
                                state.locked_x, state.locked_y = cursor_position
                            state.last_locked_scroll_point = None
                            state.locked_scroll_origin_point = None
                            state.locked_scroll_motion_started = False
                            state.locked_scroll_remainder_x = 0.0
                            state.locked_scroll_remainder_y = 0.0
                            state.last_rotate_angle = None
                            state.rotate_drag_offset = 0.0
                            state.rotate_angle_remainder = 0.0
                            state.rotate_filtered_angle = None
                            state.rotate_pose_active = False
                            state.rotate_pose_since = None
                            state.rotate_pose_last_seen = None
                            state.rotate_drag_position_x = None
                            state.gesture_count += 1

                with state.lock:
                    cursor_locked = state.cursor_locked
                    palm_hold_progress = state.palm_hold_progress
                    palm_completion_locked = state.palm_completion_locked
                    if state.palm_completion_time is not None:
                        palm_feedback_age = palm_now - state.palm_completion_time

                wrist = landmarks[0]
                thumb_tip = landmarks[4]
                idx_tip = landmarks[8]
                mid_tip = landmarks[12]
                pnk_tip = landmarks[20]

                d_idx = dist(thumb_tip, idx_tip)
                d_mid = dist(thumb_tip, mid_tip)
                d_pnk = dist(thumb_tip, pnk_tip)

                is_left = (
                    hand_mouse_active and not fist_output_blocked
                    and not cursor_locked and not palm_effective and d_idx < thresh
                )
                is_right = (
                    hand_mouse_active and not fist_output_blocked
                    and not cursor_locked and not palm_effective and d_mid < thresh
                )
                is_scroll = (
                    hand_mouse_active and not fist_output_blocked
                    and not cursor_locked and not palm_effective and d_pnk < thresh
                )
                scroll_pose = (
                    hand_mouse_active and not fist_output_blocked and cursor_locked
                    and is_locked_scroll_pose(
                        fingers, landmarks, fingers[5]
                    )
                )
                rotate_pose = (
                    hand_mouse_active and not fist_output_blocked and cursor_locked
                    and is_rotate_pose(
                        fingers, landmarks, fingers[5]
                    )
                )
                with state.lock:
                    if scroll_pose:
                        state.locked_scroll_pose_last_seen = palm_now
                        if state.locked_scroll_pose_since is None:
                            state.locked_scroll_pose_since = palm_now
                        if (
                            not state.locked_scroll_pose_active
                            and palm_now - state.locked_scroll_pose_since >= GESTURE_POSE_CONFIRMATION
                        ):
                            state.locked_scroll_pose_active = True
                    elif (
                        state.locked_scroll_pose_active
                        and state.locked_scroll_pose_last_seen is not None
                        and palm_now - state.locked_scroll_pose_last_seen <= GESTURE_TRACKING_GRACE
                    ):
                        pass
                    else:
                        state.locked_scroll_pose_active = False
                        state.locked_scroll_pose_since = None
                        state.locked_scroll_pose_last_seen = None
                    is_locked_scroll = (
                        hand_mouse_active
                        and not fist_output_blocked
                        and cursor_locked
                        and state.locked_scroll_pose_active
                    )

                    if rotate_pose:
                        state.rotate_pose_last_seen = palm_now
                        if state.rotate_pose_since is None:
                            state.rotate_pose_since = palm_now
                        if (
                            not state.rotate_pose_active
                            and palm_now - state.rotate_pose_since >= GESTURE_POSE_CONFIRMATION
                        ):
                            state.rotate_pose_active = True
                            state.rotate_drag_offset = 0.0
                            state.rotate_angle_remainder = 0.0
                            state.rotate_filtered_angle = None
                            state.last_rotate_angle = None
                    elif (
                        state.rotate_pose_active
                        and state.rotate_pose_last_seen is not None
                        and palm_now - state.rotate_pose_last_seen <= GESTURE_TRACKING_GRACE
                    ):
                        pass
                    else:
                        state.rotate_pose_active = False
                        state.rotate_pose_since = None
                        state.rotate_pose_last_seen = None
                    is_rotate = (
                        hand_mouse_active
                        and not fist_output_blocked
                        and cursor_locked
                        and state.rotate_pose_active
                    )

                # Draw skeleton
                draw_hand_skeleton(frame, landmarks)

                h_px, w_px = frame.shape[:2]
                if is_left:
                    cv2.line(frame,
                             (int(thumb_tip.x * w_px), int(thumb_tip.y * h_px)),
                             (int(idx_tip.x * w_px), int(idx_tip.y * h_px)),
                             LEFT_COLOR, 3, cv2.LINE_AA)
                if is_right:
                    cv2.line(frame,
                             (int(thumb_tip.x * w_px), int(thumb_tip.y * h_px)),
                             (int(mid_tip.x * w_px), int(mid_tip.y * h_px)),
                             RIGHT_COLOR, 3, cv2.LINE_AA)
                if is_scroll:
                    cv2.line(frame,
                             (int(thumb_tip.x * w_px), int(thumb_tip.y * h_px)),
                             (int(pnk_tip.x * w_px), int(pnk_tip.y * h_px)),
                             SCROLL_COLOR, 3, cv2.LINE_AA)

                x = remap(wrist.x, rmin, rmax)
                y = remap(wrist.y, rmin, rmax)

                with state.lock:
                    state.target_x = x * screen_width
                    state.target_y = y * screen_height
                    previous_scroll_y = state.smooth_scroll_y
                    last_scroll = state.last_scroll

                # Preserve the existing thumb+pinky scroll gesture while unlocked.
                if is_scroll:
                    if previous_scroll_y is None or not last_scroll:
                        smooth_scroll_y = y
                    else:
                        previous_smooth_y = previous_scroll_y
                        smooth_scroll_y = previous_scroll_y + (
                            y - previous_scroll_y
                        ) * state.SCROLL_SMOOTHING
                        delta = (
                            (smooth_scroll_y - previous_smooth_y)
                            * -state.SCROLL_SENSITIVITY
                        )
                        if abs(delta) > state.SCROLL_DEAD_ZONE:
                            try:
                                with state.pointer_lock:
                                    if mouse_output_enabled():
                                        pyautogui.scroll(int(delta))
                            except pyautogui.FailSafeException:
                                with state.lock:
                                    state.running = False
                                print(
                                    "PyAutoGUI emergency fail-safe triggered at a screen corner; "
                                    "gesture control stopped. Move the pointer away from the corner "
                                    "before starting again."
                                )
                                break
                    with state.lock:
                        state.smooth_scroll_y = smooth_scroll_y
                else:
                    with state.lock:
                        state.smooth_scroll_y = None

                # While locked, hand displacement scrolls in either axis.
                if is_locked_scroll:
                    scroll_point = (landmarks[9].x, landmarks[9].y)
                    with state.lock:
                        previous_point = state.last_locked_scroll_point
                        if previous_point is None:
                            smooth_point = scroll_point
                            state.locked_scroll_origin_point = scroll_point
                            state.locked_scroll_motion_started = False
                        else:
                            smooth_point = (
                                previous_point[0] + (scroll_point[0] - previous_point[0]) * 0.25,
                                previous_point[1] + (scroll_point[1] - previous_point[1]) * 0.25,
                            )
                            origin = state.locked_scroll_origin_point
                            if (
                                not state.locked_scroll_motion_started
                                and origin is not None
                                and math.hypot(
                                    smooth_point[0] - origin[0],
                                    smooth_point[1] - origin[1],
                                ) >= 0.003
                            ):
                                state.locked_scroll_motion_started = True
                        state.last_locked_scroll_point = smooth_point
                        motion_started = state.locked_scroll_motion_started
                    if previous_point is not None and motion_started:
                        delta_x = smooth_point[0] - previous_point[0]
                        delta_y = smooth_point[1] - previous_point[1]
                        with state.lock:
                            horizontal, state.locked_scroll_remainder_x = scroll_steps(
                                delta_x * state.LOCKED_SCROLL_SENSITIVITY,
                                state.locked_scroll_remainder_x,
                            )
                            vertical, state.locked_scroll_remainder_y = scroll_steps(
                                -delta_y * state.LOCKED_SCROLL_SENSITIVITY,
                                state.locked_scroll_remainder_y,
                            )
                        try:
                            with state.pointer_lock:
                                if mouse_output_enabled():
                                    if horizontal:
                                        pyautogui.hscroll(horizontal)
                                    if vertical:
                                        pyautogui.scroll(vertical)
                        except pyautogui.FailSafeException:
                            with state.lock:
                                state.running = False
                            print(
                                "PyAutoGUI emergency fail-safe triggered at a screen corner; "
                                "gesture control stopped. Move the pointer away from the corner "
                                "before starting again."
                            )
                            break
                else:
                    with state.lock:
                        state.last_locked_scroll_point = None
                        state.locked_scroll_origin_point = None
                        state.locked_scroll_motion_started = False
                        state.locked_scroll_remainder_x = 0.0
                        state.locked_scroll_remainder_y = 0.0

                # Keep one drag active for the pose, moving proportionally to wrist
                # rotation instead of sending repeated tiny click-drag-release bursts.
                if is_rotate:
                    orientation = math.atan2(
                        landmarks[17].y - landmarks[5].y,
                        landmarks[17].x - landmarks[5].x,
                    )
                    with state.lock:
                        if state.rotate_filtered_angle is None:
                            state.rotate_filtered_angle = orientation
                            state.last_rotate_angle = orientation
                            movement_angle = 0.0
                        else:
                            filter_delta = wrap_angle(
                                orientation - state.rotate_filtered_angle
                            )
                            state.rotate_filtered_angle = wrap_angle(
                                state.rotate_filtered_angle + filter_delta * 0.25
                            )
                            delta_angle = wrap_angle(
                                state.rotate_filtered_angle - state.last_rotate_angle
                            )
                            state.rotate_angle_remainder += delta_angle
                            if abs(state.rotate_angle_remainder) >= 0.005:
                                movement_angle = state.rotate_angle_remainder
                                state.rotate_angle_remainder = 0.0
                            else:
                                movement_angle = 0.0
                        state.last_rotate_angle = state.rotate_filtered_angle
                        if movement_angle:
                            state.rotate_drag_offset = max(
                                -screen_width / 3,
                                min(
                                    screen_width / 3,
                                    state.rotate_drag_offset
                                    + rotation_drag_delta(movement_angle),
                                ),
                            )
                        anchor_x = state.locked_x
                        anchor_y = state.locked_y
                        drag_active = state.rotate_drag_active
                        if not drag_active:
                            state.rotate_drag_position_x = anchor_x
                        previous_drag_x = state.rotate_drag_position_x
                        state.rotate_drag_active = True

                    anchor_x = clamp_cursor(anchor_x, screen_width)
                    anchor_y = clamp_cursor(anchor_y, screen_height)
                    drag_x = clamp_cursor(anchor_x + state.rotate_drag_offset, screen_width)
                    with state.pointer_lock:
                        try:
                            if mouse_output_enabled() and not drag_active:
                                pyautogui.moveTo(int(anchor_x), int(anchor_y))
                                pyautogui.mouseDown()
                                with state.lock:
                                    state.mouse_button_down = True
                            if mouse_output_enabled() and abs(drag_x - previous_drag_x) >= 1:
                                pyautogui.moveTo(int(drag_x), int(anchor_y))
                                with state.lock:
                                    state.rotate_drag_position_x = drag_x
                        except pyautogui.FailSafeException:
                            with state.lock:
                                state.running = False
                            print(
                                "PyAutoGUI emergency fail-safe triggered during model rotation; "
                                "gesture control stopped."
                            )
                            break
                else:
                    with state.lock:
                        release_rotate_drag = state.rotate_drag_active
                        anchor_x = state.locked_x
                        anchor_y = state.locked_y
                        state.rotate_drag_active = False
                        state.rotate_drag_position_x = None
                        state.last_rotate_angle = None
                        state.rotate_drag_offset = 0.0
                        state.rotate_angle_remainder = 0.0
                        state.rotate_filtered_angle = None
                    if release_rotate_drag:
                        with state.pointer_lock:
                            try:
                                with state.lock:
                                    release_mouse = state.mouse_button_down
                                    state.mouse_button_down = False
                                if release_mouse:
                                    pyautogui.mouseUp()
                                if mouse_output_enabled():
                                    pyautogui.moveTo(
                                        int(clamp_cursor(anchor_x, screen_width)),
                                        int(clamp_cursor(anchor_y, screen_height)),
                                    )
                            except pyautogui.FailSafeException:
                                with state.lock:
                                    state.running = False
                                print(
                                    "PyAutoGUI emergency fail-safe triggered while ending model rotation; "
                                    "gesture control stopped."
                                )
                                break

                with state.lock:
                    if is_left and not state.last_left:
                        state.gesture_count += 1
                    if is_right and not state.last_right:
                        state.gesture_count += 1
                    if is_scroll and not state.last_scroll:
                        state.gesture_count += 1
                    if is_locked_scroll and not state.gesture_locked_scroll:
                        state.gesture_count += 1
                    if is_rotate and not state.gesture_rotate:
                        state.gesture_count += 1
                    state.last_left = is_left
                    state.last_right = is_right
                    state.last_scroll = is_scroll
                    state.gesture_locked_scroll = is_locked_scroll
                    state.gesture_rotate = is_rotate

            else:
                (
                    _,
                    hand_mouse_active,
                    fist_output_blocked,
                    fist_hold_progress,
                    release_mouse,
                ) = update_fist_hold(False, False, palm_now)
                if release_mouse:
                    try:
                        release_held_mouse_button()
                    except pyautogui.FailSafeException:
                        with state.lock:
                            state.running = False
                        print(
                            "PyAutoGUI emergency fail-safe triggered while pausing hand control."
                        )
                        break
                with state.lock:
                    was_left = state.last_left
                    state.last_left = state.last_right = state.last_scroll = False
                    keep_scroll_pose = (
                        state.locked_scroll_pose_active
                        and state.locked_scroll_pose_last_seen is not None
                        and palm_now - state.locked_scroll_pose_last_seen <= GESTURE_TRACKING_GRACE
                    )
                    is_locked_scroll = (
                        state.hand_mouse_active
                        and not state.fist_output_blocked
                        and state.cursor_locked
                        and keep_scroll_pose
                    )
                    state.gesture_locked_scroll = is_locked_scroll
                    state.smooth_scroll_y = None
                    if not keep_scroll_pose:
                        state.last_locked_scroll_point = None
                        state.locked_scroll_origin_point = None
                        state.locked_scroll_motion_started = False
                        state.locked_scroll_remainder_x = 0.0
                        state.locked_scroll_remainder_y = 0.0
                        state.locked_scroll_pose_active = False
                        state.locked_scroll_pose_since = None
                        state.locked_scroll_pose_last_seen = None
                    keep_rotate_drag = (
                        state.hand_mouse_active
                        and not state.fist_output_blocked
                        and state.rotate_drag_active
                        and state.rotate_pose_last_seen is not None
                        and palm_now - state.rotate_pose_last_seen <= GESTURE_TRACKING_GRACE
                    )
                    was_rotating = state.rotate_drag_active and not keep_rotate_drag
                    is_rotate = keep_rotate_drag
                    if not keep_rotate_drag:
                        state.rotate_drag_active = False
                        state.rotate_drag_position_x = None
                        state.rotate_pose_active = False
                        state.rotate_pose_since = None
                        state.rotate_pose_last_seen = None
                        state.last_rotate_angle = None
                        state.rotate_drag_offset = 0.0
                        state.rotate_angle_remainder = 0.0
                        state.rotate_filtered_angle = None
                    state.gesture_rotate = False
                    if (
                        state.palm_gesture_since is not None
                        and state.palm_last_seen is not None
                        and palm_now - state.palm_last_seen <= PALM_TRACKING_GRACE
                    ):
                        state.palm_gesture_active = True
                        state.palm_hold_progress = min(
                            (palm_now - state.palm_gesture_since) / PALM_HOLD_SECONDS, 1.0
                        )
                    else:
                        state.palm_gesture_active = False
                        state.palm_gesture_since = None
                        state.palm_last_seen = None
                        state.palm_toggle_armed = True
                        state.palm_hold_progress = 0.0
                    cursor_locked = state.cursor_locked
                    palm_hold_progress = state.palm_hold_progress
                    palm_completion_locked = state.palm_completion_locked
                    if state.palm_completion_time is not None:
                        palm_feedback_age = palm_now - state.palm_completion_time
                if was_left:
                    with state.pointer_lock:
                        with state.lock:
                            release_mouse = state.mouse_button_down
                        if release_mouse:
                            pyautogui.mouseUp()
                            with state.lock:
                                state.mouse_button_down = False
                if was_rotating:
                    with state.pointer_lock:
                        try:
                            with state.lock:
                                release_mouse = state.mouse_button_down
                                state.mouse_button_down = False
                                anchor_x = state.locked_x
                                anchor_y = state.locked_y
                            if release_mouse:
                                pyautogui.mouseUp()
                            if mouse_output_enabled():
                                pyautogui.moveTo(
                                    int(clamp_cursor(anchor_x, screen_width)),
                                    int(clamp_cursor(anchor_y, screen_height)),
                                )
                        except pyautogui.FailSafeException:
                            with state.lock:
                                state.running = False
                            print(
                                "PyAutoGUI emergency fail-safe triggered while ending model rotation; "
                                "gesture control stopped."
                            )
                            break

            with state.lock:
                state.hand_detected  = hand_detected
                state.gesture_left   = is_left
                state.gesture_right  = is_right
                state.gesture_scroll = is_scroll
                state.gesture_locked_scroll = is_locked_scroll
                state.gesture_rotate = is_rotate

            # Draw HUD
            draw_overlay(
                frame, hand_detected, is_left, is_right, is_scroll,
                is_locked_scroll, is_rotate, cursor_locked, palm_hold_progress,
                palm_completion_locked, palm_feedback_age, landmarks, fps,
                hand_mouse_active, fist_hold_progress,
            )

            # Push frame to queue (drop old frame if full)
            if state.frame_queue.full():
                try:
                    state.frame_queue.get_nowait()
                except queue.Empty:
                    pass
            try:
                state.frame_queue.put_nowait(frame.copy())
            except queue.Full:
                pass

# ── 60fps cursor update thread ────────────────────────────────────────────────
def cursor_update_thread():
    try:
        _cursor_update_loop()
    except pyautogui.FailSafeException:
        with state.lock:
            state.running = False
        print("PyAutoGUI fail-safe triggered; stopping gesture control.")
    except Exception as error:
        with state.lock:
            state.running = False
        print(f"ERROR: Cursor control stopped: {error}")
    finally:
        with state.lock:
            release_mouse = state.mouse_button_down
            state.mouse_button_down = False
        if release_mouse:
            failsafe = pyautogui.FAILSAFE
            pyautogui.FAILSAFE = False
            try:
                pyautogui.mouseUp()
            except Exception as error:
                print(f"ERROR: Could not release mouse button: {error}")
            finally:
                pyautogui.FAILSAFE = failsafe


def _cursor_update_loop():
    """Runs at ~60 Hz, applies One Euro Filter, and moves the cursor.
    Decoupled from camera frame rate for fluid motion."""
    filter_x = OneEuroFilter(freq=60.0, min_cutoff=1.5, beta=0.01)
    filter_y = OneEuroFilter(freq=60.0, min_cutoff=1.5, beta=0.01)
    interval = 1.0 / 60.0  # ~16.67 ms

    prev_left = False
    prev_right = False

    while state.running:
        t = time.time()

        with state.lock:
            hand = state.hand_detected
            tx = state.target_x
            ty = state.target_y
            is_left = state.gesture_left
            is_right = state.gesture_right
            is_scroll = state.gesture_scroll
            smoothing = state.smoothing
            cursor_locked = state.cursor_locked
            palm_active = state.palm_gesture_active
            hand_mouse_active = state.hand_mouse_active
            fist_output_blocked = state.fist_output_blocked

        if (
            hand and hand_mouse_active and not fist_output_blocked
            and not is_scroll and not cursor_locked and not palm_active
        ):
            # Update filter min_cutoff from smoothing slider
            # smoothing 0.05→0.5 maps to min_cutoff 3.0→0.3 (lower = smoother)
            mc = max(0.3, 3.0 - smoothing * 6.0)
            filter_x.min_cutoff = mc
            filter_y.min_cutoff = mc

            fx = filter_x(tx, t)
            fy = filter_y(ty, t)
            fx = clamp_cursor(fx, screen_width)
            fy = clamp_cursor(fy, screen_height)

            with state.lock:
                state.curr_x = fx
                state.curr_y = fy

            with state.pointer_lock:
                with state.lock:
                    can_move_cursor = (
                        state.hand_mouse_active
                        and not state.fist_output_blocked
                        and not state.cursor_locked
                        and not state.palm_gesture_active
                    )
                if can_move_cursor:
                    pyautogui.moveTo(int(fx), int(fy))

            if is_left and not prev_left:
                with state.pointer_lock:
                    if mouse_output_enabled():
                        pyautogui.mouseDown()
                        with state.lock:
                            state.mouse_button_down = True
            elif not is_left and prev_left:
                with state.pointer_lock:
                    with state.lock:
                        release_mouse = state.mouse_button_down
                    if release_mouse:
                        pyautogui.mouseUp()
                        with state.lock:
                            state.mouse_button_down = False

            if is_right and not prev_right:
                with state.pointer_lock:
                    if mouse_output_enabled():
                        pyautogui.click(button="right")

            prev_left = is_left
            prev_right = is_right
        else:
            if prev_left:
                with state.pointer_lock:
                    with state.lock:
                        release_mouse = state.mouse_button_down
                    if release_mouse:
                        pyautogui.mouseUp()
                        with state.lock:
                            state.mouse_button_down = False
            prev_left = False
            prev_right = False
            filter_x.reset()
            filter_y.reset()

        elapsed = time.time() - t
        sleep_time = interval - elapsed
        if sleep_time > 0:
            time.sleep(sleep_time)


# ── Always-on-top tkinter camera window ──────────────────────────────────────
def run_camera_window():
    """Runs the always-on-top floating camera preview using tkinter.
    MUST be called from the main thread."""
    import tkinter as tk
    from PIL import Image, ImageTk

    root = tk.Tk()
    root.title("Vision Mode — Camera")
    root.attributes("-topmost", True)
    root.attributes("-alpha", 0.95)
    root.resizable(True, True)
    root.configure(bg="#0f0f13")

    # Window geometry: small preview, bottom-right
    win_w, win_h = 320, 260
    sw = root.winfo_screenwidth()
    sh = root.winfo_screenheight()
    root.geometry(f"{win_w}x{win_h}+{sw - win_w - 20}+{sh - win_h - 60}")

    # Title bar
    title_bar = tk.Frame(root, bg="#1a1a24", height=28)
    title_bar.pack(fill=tk.X, side=tk.TOP)
    title_bar.pack_propagate(False)

    tk.Label(title_bar, text="● LIVE  Vision Mode",
             bg="#1a1a24", fg="#a29bfe",
             font=("Segoe UI", 9, "bold")).pack(side=tk.LEFT, padx=8)

    # Minimize/restore button
    minimized = [False]
    def toggle_size():
        if minimized[0]:
            root.geometry(f"{win_w}x{win_h}")
            minimized[0] = False
        else:
            root.geometry(f"{win_w}x30")
            minimized[0] = True

    tk.Button(title_bar, text="—", bg="#2a2a3d", fg="white",
              relief=tk.FLAT, font=("Segoe UI", 9),
              command=toggle_size, cursor="hand2",
              padx=6).pack(side=tk.RIGHT, padx=4, pady=2)

    # Canvas for video
    canvas = tk.Canvas(root, bg="#0f0f13", highlightthickness=0)
    canvas.pack(fill=tk.BOTH, expand=True)

    # Drag support
    drag = {"x": 0, "y": 0}
    def on_drag_start(e):
        drag["x"] = e.x_root - root.winfo_x()
        drag["y"] = e.y_root - root.winfo_y()
    def on_drag_motion(e):
        root.geometry(f"+{e.x_root - drag['x']}+{e.y_root - drag['y']}")
    title_bar.bind("<ButtonPress-1>", on_drag_start)
    title_bar.bind("<B1-Motion>", on_drag_motion)

    # Clean exit handlers (Escape key or window close button)
    def on_close():
        state.running = False
        root.destroy()

    root.protocol("WM_DELETE_WINDOW", on_close)
    root.bind("<Escape>", lambda e: on_close())

    photo_ref = [None]

    def update_frame():
        if not state.running:
            root.destroy()
            return
        try:
            frame = state.frame_queue.get_nowait()
            cw = canvas.winfo_width()
            ch = canvas.winfo_height()
            if cw > 1 and ch > 1:
                frame_rgb = cv2.cvtColor(frame, cv2.COLOR_BGR2RGB)
                img = Image.fromarray(frame_rgb)
                img = img.resize((cw, ch), Image.LANCZOS)
                photo = ImageTk.PhotoImage(img)
                canvas.create_image(0, 0, anchor=tk.NW, image=photo)
                photo_ref[0] = photo  # prevent GC
        except queue.Empty:
            pass
        root.after(16, update_frame)  # ~60 fps

    root.after(100, update_frame)
    root.mainloop()


# ── Browser dashboard HTML ────────────────────────────────────────────────────
HTML_CONTENT = """<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Vision Mode — Dashboard</title>
  <style>
    *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
    :root {
      --bg: #08080c; --bg2: #111118; --bg3: #16161f;
      --card: rgba(22,22,34,0.6); --card-border: rgba(108,92,231,0.15);
      --glass: rgba(22,22,34,0.45);
      --border: #1e1e30; --accent: #7c6cf0; --accent-l: #a89eff;
      --accent-glow: rgba(124,108,240,0.35);
      --success: #34d9a3; --warn: #f9c74f; --blue: #60a5fa;
      --danger: #f87171; --text: #eeeef5; --muted: #7e7e9a;
      --dim: #4a4a60; --r: 12px;
    }

    body {
      background: var(--bg); color: var(--text);
      font-family: Inter, -apple-system, 'Segoe UI', sans-serif;
      height: 100vh; overflow: hidden; display: flex; flex-direction: column;
    }

    /* Subtle animated background glow */
    body::before {
      content: ''; position: fixed; top: -40%; left: -10%; width: 60%; height: 80%;
      background: radial-gradient(ellipse, rgba(124,108,240,0.06) 0%, transparent 70%);
      pointer-events: none; z-index: 0; animation: bgShift 20s ease-in-out infinite alternate;
    }
    body::after {
      content: ''; position: fixed; bottom: -30%; right: -10%; width: 50%; height: 70%;
      background: radial-gradient(ellipse, rgba(52,217,163,0.04) 0%, transparent 70%);
      pointer-events: none; z-index: 0; animation: bgShift 15s ease-in-out infinite alternate-reverse;
    }
    @keyframes bgShift { from { transform: translate(0,0); } to { transform: translate(40px, 20px); } }
    @keyframes pulse { 0%,100% { transform: scale(1); opacity:1; } 50% { transform: scale(1.5); opacity:0.5; } }
    @keyframes fadeIn { from { opacity:0; transform: translateY(12px); } to { opacity:1; transform: none; } }
    @keyframes heroFloat {
      0%,100% { transform: translateY(0); }
      50% { transform: translateY(-8px); }
    }

    /* Header */
    .hdr {
      display: flex; align-items: center; justify-content: space-between;
      padding: 14px 28px; background: var(--bg2); position: relative; z-index: 10;
      border-bottom: 1px solid var(--border);
      backdrop-filter: blur(12px);
    }
    .brand { display: flex; align-items: center; gap: 12px; }
    .brand svg { width: 26px; height: 26px; color: var(--accent); filter: drop-shadow(0 0 6px var(--accent-glow)); }
    .brand h1 { font-size: 16px; font-weight: 700; letter-spacing: -0.3px; }
    .brand span { background: linear-gradient(135deg, var(--accent), var(--accent-l));
                  -webkit-background-clip: text; -webkit-text-fill-color: transparent; }
    .pill {
      display: flex; align-items: center; gap: 8px; font-size: 12px; font-weight: 500;
      padding: 6px 16px; border-radius: 20px;
      background: var(--glass); border: 1px solid var(--card-border);
      backdrop-filter: blur(10px);
    }
    .dot { width: 8px; height: 8px; border-radius: 50%; background: var(--dim);
           transition: all 0.3s ease; }
    .dot.ok  { background: var(--success); box-shadow: 0 0 10px rgba(52,217,163,0.5);
               animation: pulse 2s infinite; }
    .dot.err { background: var(--danger); box-shadow: 0 0 10px rgba(248,113,113,0.5); }

    /* Layout */
    .main {
      flex: 1; display: flex; align-items: flex-start; justify-content: center;
      overflow-y: auto; position: relative; z-index: 1;
      padding: 40px 24px;
    }

    .dashboard {
      width: 100%; max-width: 820px;
      display: flex; flex-direction: column; gap: 24px;
      animation: fadeIn 0.6s ease-out;
    }

    /* Hero banner */
    .hero-banner {
      display: flex; align-items: center; gap: 24px;
      background: var(--card); border: 1px solid var(--card-border);
      border-radius: 16px; padding: 28px 32px;
      backdrop-filter: blur(10px);
    }
    .hero {
      width: 64px; height: 64px; border-radius: 50%; flex-shrink: 0;
      background: linear-gradient(135deg, var(--accent), #9f6cf0, var(--accent-l));
      display: flex; align-items: center; justify-content: center;
      box-shadow: 0 0 40px var(--accent-glow);
      animation: heroFloat 4s ease-in-out infinite;
      position: relative;
    }
    .hero::before {
      content: ''; position: absolute; inset: -3px; border-radius: 50%;
      background: linear-gradient(135deg, var(--accent), transparent, var(--accent-l));
      opacity: 0.3; filter: blur(6px);
    }
    .hero svg { width: 30px; height: 30px; color: #fff; position: relative; z-index: 1; }
    .hero-text { flex: 1; }
    .hero-title { font-size: 22px; font-weight: 700; letter-spacing: -0.5px; margin-bottom: 6px; }
    .hero-sub { font-size: 13px; color: var(--muted); line-height: 1.6; font-weight: 300; }

    /* Two-column grid */
    .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 20px; }

    /* Cards */
    .card {
      background: var(--card); border: 1px solid var(--card-border);
      border-radius: 14px; padding: 22px 24px;
      backdrop-filter: blur(10px);
    }
    .card-title {
      font-size: 10px; font-weight: 700; text-transform: uppercase;
      letter-spacing: 1.5px; color: var(--dim); margin-bottom: 16px;
    }

    /* Gesture cards */
    .g-list { display: flex; flex-direction: column; gap: 8px; }
    .g-item {
      display: flex; align-items: center; gap: 10px; padding: 10px 14px;
      border-radius: 10px; background: rgba(8,8,14,0.4); border: 1px solid var(--card-border);
      transition: all 0.3s cubic-bezier(0.4, 0, 0.2, 1); cursor: default;
    }
    .g-item:hover { background: rgba(30,30,50,0.5); transform: translateX(3px); }
    .g-item.on {
      border-color: var(--accent); background: rgba(124,108,240,0.1);
      box-shadow: 0 0 20px rgba(124,108,240,0.12), inset 0 0 20px rgba(124,108,240,0.05);
      transform: translateX(3px);
    }
    .g-dot { width: 10px; height: 10px; border-radius: 50%; flex-shrink: 0;
             transition: all 0.3s ease; }
    .g-dot.lc { background: var(--warn); }
    .g-dot.rc { background: var(--blue); }
    .g-dot.sc { background: var(--accent-l); }
    .g-item.on .g-dot { box-shadow: 0 0 12px currentColor; animation: pulse 1.5s infinite; }
    .g-item.on .g-dot.lc { color: var(--warn); }
    .g-item.on .g-dot.rc { color: var(--blue); }
    .g-item.on .g-dot.sc { color: var(--accent-l); }
    .g-name { font-size: 13px; font-weight: 500; }
    .g-key {
      margin-left: auto; font-size: 9px; font-weight: 600; color: var(--dim);
      padding: 3px 8px; border-radius: 5px; background: rgba(8,8,14,0.6);
      letter-spacing: 0.3px; text-transform: uppercase;
    }

    /* Stats row */
    .stats-row { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; margin-top: 14px; }
    .stat {
      background: rgba(8,8,14,0.4); border: 1px solid var(--card-border);
      border-radius: 10px; padding: 14px 16px; text-align: center;
      transition: all 0.3s ease;
    }
    .stat:hover { border-color: var(--accent); transform: translateY(-2px);
                  box-shadow: 0 6px 20px rgba(0,0,0,0.3); }
    .stat-v { font-size: 24px; font-weight: 800; color: var(--accent-l);
              font-variant-numeric: tabular-nums; }
    .stat-l { font-size: 9px; color: var(--dim); text-transform: uppercase;
              letter-spacing: 1px; margin-top: 3px; font-weight: 600; }

    /* Sliders */
    .ctrl { margin-bottom: 16px; }
    .ctrl:last-child { margin-bottom: 0; }
    .ctrl-hdr { display: flex; justify-content: space-between; margin-bottom: 8px; }
    .ctrl-lbl { font-size: 12px; color: var(--muted); font-weight: 400; }
    .ctrl-val { font-size: 12px; font-weight: 700; color: var(--accent-l);
                font-variant-numeric: tabular-nums; }

    input[type=range] {
      -webkit-appearance: none; width: 100%; height: 4px;
      border-radius: 4px; background: rgba(8,8,14,0.6); outline: none; cursor: pointer;
      transition: background 0.2s;
    }
    input[type=range]:hover { background: #1a1a2a; }
    input[type=range]::-webkit-slider-thumb {
      -webkit-appearance: none; width: 18px; height: 18px;
      border-radius: 50%; background: linear-gradient(135deg, var(--accent), var(--accent-l));
      border: 2px solid var(--bg2); cursor: pointer;
      box-shadow: 0 0 10px var(--accent-glow);
      transition: transform 0.15s, box-shadow 0.15s;
    }
    input[type=range]::-webkit-slider-thumb:hover {
      transform: scale(1.2); box-shadow: 0 0 16px var(--accent-glow);
    }

    /* Performance card */
    .perf-stat { text-align: center; padding: 18px 0; }
    .perf-stat .stat-v { font-size: 36px; }

    /* Notice */
    .notice {
      background: var(--card); border: 1px solid var(--card-border);
      border-radius: 12px; padding: 14px 20px; font-size: 12px;
      color: var(--muted); line-height: 1.6; backdrop-filter: blur(8px);
      font-weight: 300; text-align: center;
    }

    /* Footer */
    .ftr {
      padding: 8px 28px; background: var(--bg2); border-top: 1px solid var(--border);
      display: flex; justify-content: space-between; font-size: 10px;
      color: var(--dim); font-weight: 500; letter-spacing: 0.3px; z-index: 10;
    }

    /* Scrollbar */
    ::-webkit-scrollbar { width: 4px; }
    ::-webkit-scrollbar-track { background: transparent; }
    ::-webkit-scrollbar-thumb { background: var(--dim); border-radius: 4px; }
    ::-webkit-scrollbar-thumb:hover { background: var(--accent); }

    @media (max-width: 700px) {
      .hdr { padding: 12px 16px; }
      .main { padding: 20px 16px; }
      .dashboard { gap: 16px; }
      .hero-banner { gap: 16px; padding: 20px; }
      .hero-title { font-size: 18px; }
      .grid { grid-template-columns: 1fr; gap: 16px; }
      .card { padding: 18px; }
      .ftr { padding: 8px 16px; }
    }
  </style>
</head>
<body>
  <header class="hdr">
    <div class="brand">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
           stroke-linecap="round" stroke-linejoin="round">
        <path d="M18 11V6a2 2 0 0 0-4 0v0M14 10V4a2 2 0 0 0-4 0v2M10 10.5V6a2 2 0 0 0-4 0v8"/>
        <path d="M18 8a2 2 0 1 1 4 0v6a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15"/>
      </svg>
      <h1>Gesture<span>Mouse</span></h1>
    </div>
    <div class="pill">
      <div class="dot" id="dot"></div>
      <span id="statusTxt">Connecting...</span>
    </div>
  </header>

  <div class="main">
    <div class="dashboard">

      <!-- Hero banner -->
      <div class="hero-banner">
        <div class="hero">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
               stroke-linecap="round" stroke-linejoin="round">
            <path d="M18 11V6a2 2 0 0 0-4 0v0M14 10V4a2 2 0 0 0-4 0v2M10 10.5V6a2 2 0 0 0-4 0v8"/>
            <path d="M18 8a2 2 0 1 1 4 0v6a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15"/>
          </svg>
        </div>
        <div class="hero-text">
          <h2 class="hero-title">Vision Mode</h2>
          <p class="hero-sub">
            Move your hand in front of the camera to control the cursor.
            Hold an open palm for five seconds to lock or unlock the cursor.
            To rotate a 3D view that supports mouse dragging, place the pointer
            over the model before locking, then turn your wrist with thumb and
            index together or extended and the other fingers curled. Move two
            joined fingers while locked to scroll.
          </p>
        </div>
      </div>

      <!-- Two-column grid -->
      <div class="grid">

        <!-- Left column: Gestures + Stats -->
        <div style="display:flex; flex-direction:column; gap:20px;">
          <div class="card">
            <div class="card-title">Active Gestures</div>
            <div class="g-list">
              <div class="g-item" id="gL">
                <div class="g-dot lc"></div>
                <span class="g-name">Left Click</span>
                <span class="g-key">Thumb + Index</span>
              </div>
              <div class="g-item" id="gR">
                <div class="g-dot rc"></div>
                <span class="g-name">Right Click</span>
                <span class="g-key">Thumb + Middle</span>
              </div>
              <div class="g-item" id="gS">
                <div class="g-dot sc"></div>
                <span class="g-name">Scroll</span>
                <span class="g-key">Thumb + Pinky</span>
              </div>
              <div class="g-item" id="gLock">
                <div class="g-dot sc"></div>
                <span class="g-name">Cursor Lock</span>
                <span class="g-key">Open palm · 5 seconds</span>
              </div>
              <div class="g-item" id="gLS">
                <div class="g-dot sc"></div>
                <span class="g-name">Directional Scroll</span>
                <span class="g-key">Index + middle together</span>
              </div>
              <div class="g-item" id="gRotate">
                <div class="g-dot lc"></div>
                <span class="g-name">3D Rotate</span>
                <span class="g-key">Thumb + index · turn wrist</span>
              </div>
            </div>
            <div class="stats-row">
              <div class="stat">
                <div class="stat-v" id="gestureCount">0</div>
                <div class="stat-l">Gestures</div>
              </div>
              <div class="stat">
                <div class="stat-v" id="handStatus">—</div>
                <div class="stat-l">Hand</div>
              </div>
            </div>
          </div>
        </div>

        <!-- Right column: Settings + Performance -->
        <div style="display:flex; flex-direction:column; gap:20px;">
          <div class="card">
            <div class="card-title">Settings</div>
            <div class="ctrl">
              <div class="ctrl-hdr">
                <span class="ctrl-lbl">Pinch Threshold</span>
                <span class="ctrl-val" id="threshVal">0.05</span>
              </div>
              <input type="range" id="pinchThreshold" min="0.02" max="0.15" step="0.01" value="0.05">
            </div>
            <div class="ctrl">
              <div class="ctrl-hdr">
                <span class="ctrl-lbl">Smoothing</span>
                <span class="ctrl-val" id="smoothVal">0.30</span>
              </div>
              <input type="range" id="smoothing" min="0.05" max="0.5" step="0.05" value="0.3">
            </div>
            <div class="ctrl">
              <div class="ctrl-hdr">
                <span class="ctrl-lbl">Sensitivity</span>
                <span class="ctrl-val" id="sensVal">1.00</span>
              </div>
              <input type="range" id="sensitivity" min="0.5" max="2.5" step="0.1" value="1.0">
            </div>
          </div>

          <div class="card">
            <div class="card-title">Performance</div>
            <div class="perf-stat">
              <div class="stat-v" id="fpsVal">--</div>
              <div class="stat-l">Frames / sec</div>
            </div>
          </div>
        </div>

      </div>

      <!-- Notice -->
      <div class="notice">
        📷 Camera detection runs natively in Python — it stays active even when this browser window is minimized.
      </div>

    </div>
  </div>

  <footer class="ftr">
    <span>MediaPipe · OpenCV · Python</span>
    <span>v3.0</span>
  </footer>

  <script>
    const dot = document.getElementById('dot');
    const statusTxt = document.getElementById('statusTxt');
    const gL = document.getElementById('gL');
    const gR = document.getElementById('gR');
    const gS = document.getElementById('gS');
    const gLock = document.getElementById('gLock');
    const gLS = document.getElementById('gLS');
    const gRotate = document.getElementById('gRotate');
    const fpsVal = document.getElementById('fpsVal');
    const gestureCount = document.getElementById('gestureCount');
    const handStatus = document.getElementById('handStatus');

    const pinchSlider = document.getElementById('pinchThreshold');
    const smoothSlider = document.getElementById('smoothing');
    const sensSlider = document.getElementById('sensitivity');
    const threshVal = document.getElementById('threshVal');
    const smoothVal = document.getElementById('smoothVal');
    const sensVal = document.getElementById('sensVal');

    pinchSlider.oninput = () => {
      threshVal.textContent = parseFloat(pinchSlider.value).toFixed(2);
      sendSettings();
    };
    smoothSlider.oninput = () => {
      smoothVal.textContent = parseFloat(smoothSlider.value).toFixed(2);
      sendSettings();
    };
    sensSlider.oninput = () => {
      sensVal.textContent = parseFloat(sensSlider.value).toFixed(2);
      sendSettings();
    };

    let ws, reconnects = 0, reconnectTimer;

    function connect() {
      if (ws && (ws.readyState === WebSocket.CONNECTING || ws.readyState === WebSocket.OPEN)) return;
      const socket = new WebSocket('ws://127.0.0.1:8765/ws');
      ws = socket;
      socket.onopen = () => {
        clearTimeout(reconnectTimer);
        reconnectTimer = null;
        reconnects = 0;
        dot.className = 'dot ok';
        statusTxt.textContent = 'Connected';
        sendSettings();
      };
      socket.onclose = () => {
        if (ws !== socket) return;
        dot.className = 'dot err';
        statusTxt.textContent = 'Reconnecting...';
        const delay = Math.min(1000 * (2 ** Math.min(reconnects, 5)), 30000);
        reconnects++;
        reconnectTimer = setTimeout(connect, delay);
      };
      socket.onerror = e => console.error(e);
      socket.onmessage = msg => {
        try {
          const d = JSON.parse(msg.data);
          if (d.type === 'status') {
            gL.classList.toggle('on', d.left);
            gR.classList.toggle('on', d.right);
            gS.classList.toggle('on', d.scroll);
            gLock.classList.toggle('on', d.cursor_locked);
            gLS.classList.toggle('on', d.locked_scroll);
            gRotate.classList.toggle('on', d.rotate);
            fpsVal.textContent = d.fps;
            gestureCount.textContent = d.gesture_count;
            handStatus.textContent = d.hand ? 'YES' : 'NO';
          }
        } catch(e) {}
      };
    }
    connect();

    function sendSettings() {
      if (ws && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({
          type: 'settings',
          pinch_threshold: parseFloat(pinchSlider.value),
          smoothing: parseFloat(smoothSlider.value),
          sensitivity: parseFloat(sensSlider.value)
        }));
      }
    }
  </script>
</body>
</html>
"""


# ── WebSocket handler ─────────────────────────────────────────────────────────
async def websocket_handler(request):
    ws = web.WebSocketResponse()
    await ws.prepare(request)

    with state.lock:
        state.ws_clients.add(ws)
        settings_payload = {
            "type": "settings",
            "pinch_threshold": state.pinch_threshold,
            "smoothing": state.smoothing,
            "sensitivity": state.sensitivity,
        }
    print("Dashboard client connected.")

    try:
        await ws.send_str(json.dumps(settings_payload))
        async for msg in ws:
            if msg.type == web.WSMsgType.TEXT:
                try:
                    data = json.loads(msg.data)
                    if data.get('type') == 'settings':
                        with state.lock:
                            if 'pinch_threshold' in data:
                                state.pinch_threshold = float(data['pinch_threshold'])
                            if 'smoothing' in data:
                                state.smoothing = float(data['smoothing'])
                            if 'sensitivity' in data:
                                state.sensitivity = float(data['sensitivity'])
                                state._update_hand_range()
                except Exception as e:
                    print(f"WS message error: {e}")
            elif msg.type == web.WSMsgType.ERROR:
                print(f"WS error: {ws.exception()}")
    except Exception as e:
        print(f"WS connection error: {e}")
    finally:
        with state.lock:
            state.ws_clients.discard(ws)
        print("Dashboard client disconnected.")

    return ws


async def index(request):
    return web.Response(text=HTML_CONTENT, content_type='text/html')


async def health(request):
    with state.lock:
        ready = state.running and state.camera_ready
    if not ready:
        return web.Response(status=503, text='camera_not_ready')
    return web.Response(text='ok')


# ── Status broadcast task ─────────────────────────────────────────────────────
async def broadcast_status():
    """Push gesture/status updates to all connected dashboard clients."""
    while True:
        await asyncio.sleep(0.1)  # 10 Hz
        with state.lock:
            clients = set(state.ws_clients)
            payload = json.dumps({
                "type": "status",
                "hand": state.hand_detected,
                "left": state.gesture_left,
                "right": state.gesture_right,
                "scroll": state.gesture_scroll,
                "cursor_locked": state.cursor_locked,
                "locked_scroll": state.gesture_locked_scroll,
                "rotate": state.gesture_rotate,
                "palm_hold_progress": state.palm_hold_progress,
                "hand_mouse_active": state.hand_mouse_active,
                "fist_hold_progress": state.fist_hold_progress,
                "fist_hold_remaining": max(
                    0.0,
                    FIST_HOLD_MS / 1000.0
                    - (time.monotonic() - state.fist_gesture_since),
                )
                if state.fist_gesture_since is not None and state.fist_toggle_armed
                else FIST_HOLD_MS / 1000.0,
                "fist_output_blocked": state.fist_output_blocked,
                "fist_toggle_armed": state.fist_toggle_armed,
                "fps": state.fps,
                "gesture_count": state.gesture_count,
            })
        for ws in clients:
            try:
                await ws.send_str(payload)
            except Exception:
                pass


# ── Asyncio server (runs in a background thread) ──────────────────────────────
def open_browser():
    time.sleep(2.0)
    webbrowser.open('http://localhost:8765')


async def server_main():
    app_web = web.Application()
    app_web.router.add_get('/', index)
    app_web.router.add_get('/health', health)
    app_web.router.add_get('/ws', websocket_handler)

    runner = web.AppRunner(app_web)
    await runner.setup()
    site = web.TCPSite(runner, '127.0.0.1', 8765)
    await site.start()

    asyncio.create_task(broadcast_status())

    try:
        await asyncio.Future()
    except asyncio.CancelledError:
        pass
    finally:
        await runner.cleanup()


def run_server_thread():
    """Run the aiohttp server in a dedicated thread with its own event loop."""
    loop = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)
    try:
        loop.run_until_complete(server_main())
    except Exception as e:
        print(f"Server error: {e}")
    finally:
        loop.close()


def managed_shutdown_listener():
    while state.running:
        command = sys.stdin.readline()
        if not command or command.strip() == "stop":
            state.running = False
            return


# ── Main entry point ──────────────────────────────────────────────────────────
if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Vision Mode cursor control")
    parser.add_argument('--no-browser', action='store_true',
                        help='Do not auto-open the dashboard in the browser')
    parser.add_argument('--managed', action='store_true',
                        help='Accept a stop command from the Macha process manager')
    args = parser.parse_args()

    print("=" * 52)
    print("  Vision Mode v3")
    print("=" * 52)
    print(f"  Dashboard : http://localhost:8765")
    print(f"  Screen    : {screen_width}x{screen_height}")
    print("  Camera window is always-on-top (native)")
    print("  Press Ctrl+C or Esc to stop")
    print("=" * 52)

    # Start hand detection in background thread
    det_thread = threading.Thread(target=hand_detection_thread, daemon=True)
    det_thread.start()

    # Start 60fps cursor update thread
    cur_thread = threading.Thread(target=cursor_update_thread, daemon=True)
    cur_thread.start()

    # Start aiohttp server in background thread
    srv_thread = threading.Thread(target=run_server_thread, daemon=True)
    srv_thread.start()

    if args.managed:
        threading.Thread(target=managed_shutdown_listener, daemon=True).start()

    # Open browser in background thread (unless --no-browser)
    if not args.no_browser:
        threading.Thread(target=open_browser, daemon=True).start()

    # Run tkinter camera window on the MAIN thread (required by Tcl/Tk)
    try:
        run_camera_window()
    except KeyboardInterrupt:
        pass
    finally:
        state.running = False
        print("\nServer stopped.")
