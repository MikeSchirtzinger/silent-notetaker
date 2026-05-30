#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["websockets>=13.0", "anthropic>=0.40.0"]
# ///
#
# Silent Notetaker — Claude Bridge Server
#
# Connects the browser-based meeting notetaker to Claude for:
#   - Enhanced transcript categorization (better than regex triggers)
#   - Screenshot/slide analysis via vision
#   - Smart meeting summaries
#   - Ad-hoc queries during meetings
#
# Auth priority:
#   1. macOS Keychain (Claude Code OAuth credentials)
#   2. ANTHROPIC_API_KEY environment variable
#   3. ~/.config/silent-notetaker/token file
#
# Run:    uv run bridge.py
# Port:   ws://localhost:8765

import asyncio
import json
import base64
import os
import subprocess
import logging
import sys
from pathlib import Path
from datetime import datetime

import websockets
import anthropic

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
)
log = logging.getLogger("bridge")

FAST_MODEL = "claude-sonnet-4-6"   # Real-time transcript analysis, screenshots, queries
SMART_MODEL = "claude-sonnet-4-6"  # Meeting summaries (swap to claude-opus-4 for production)

TOKEN_DIR = Path.home() / ".config" / "silent-notetaker"
TOKEN_FILE = TOKEN_DIR / "token"


def get_keychain_oauth_token() -> str | None:
    """Try to read Claude Code's OAuth credentials from macOS Keychain."""
    if sys.platform != "darwin":
        return None
    try:
        result = subprocess.run(
            ["security", "find-generic-password", "-s", "Claude Code-credentials", "-w"],
            capture_output=True, text=True, timeout=5,
        )
        if result.returncode == 0 and result.stdout.strip():
            raw = result.stdout.strip()
            # Claude Code stores JSON with accessToken/refreshToken
            try:
                creds = json.loads(raw)
                # Claude Code nests under claudeAiOauth
                oauth = creds.get("claudeAiOauth", creds)
                token = oauth.get("accessToken") or oauth.get("access_token")
                if token:
                    log.info("Auth: Using Claude Code OAuth token from macOS Keychain")
                    return token
            except json.JSONDecodeError:
                # Might be a raw token string
                if len(raw) > 20:
                    log.info("Auth: Using raw token from macOS Keychain")
                    return raw
    except (subprocess.TimeoutExpired, FileNotFoundError):
        pass
    return None


def get_saved_token() -> str | None:
    """Read token from local config file."""
    if TOKEN_FILE.exists():
        token = TOKEN_FILE.read_text().strip()
        if token:
            log.info(f"Auth: Using saved token from {TOKEN_FILE}")
            return token
    return None


def save_token(token: str) -> None:
    """Save token to local config for future use."""
    TOKEN_DIR.mkdir(parents=True, exist_ok=True)
    TOKEN_FILE.write_text(token)
    TOKEN_FILE.chmod(0o600)
    log.info(f"Auth: Token saved to {TOKEN_FILE}")


def create_client() -> anthropic.Anthropic:
    """Create Anthropic client using best available auth method."""
    # 1. Try macOS Keychain (Claude Code OAuth)
    oauth_token = get_keychain_oauth_token()
    if oauth_token:
        return anthropic.Anthropic(auth_token=oauth_token)

    # 2. Try saved token file
    saved = get_saved_token()
    if saved:
        return anthropic.Anthropic(api_key=saved)

    # 4. Interactive prompt — ask user for API key, save it
    log.warning("No auth credentials found.")
    log.info("Options:")
    log.info("  1. Log into Claude Code first (claude login) — credentials are shared via Keychain")
    log.info("  2. Set ANTHROPIC_API_KEY in your shell profile")
    log.info("  3. Enter an API key now (will be saved locally)")
    log.info("")

    try:
        key = input("Paste your Anthropic API key (or press Enter to abort): ").strip()
        if key:
            save_token(key)
            return anthropic.Anthropic(api_key=key)
    except (EOFError, KeyboardInterrupt):
        pass

    log.error("No auth credentials available. Exiting.")
    sys.exit(1)


client = create_client()

ANALYSIS_INTERVAL = 5  # Analyze every N transcript chunks


# ---------------------------------------------------------------------------
# Meeting context (per-connection)
# ---------------------------------------------------------------------------

class MeetingContext:
    def __init__(self):
        self.transcript_chunks: list[dict] = []
        self.screenshot_analyses: list[dict] = []
        self.start_time = datetime.now()

    def add_chunk(self, text: str, timestamp: int) -> None:
        self.transcript_chunks.append({"text": text, "timestamp": timestamp})

    def get_recent_context(self, n: int = 10) -> str:
        """Return the last N chunks joined into a single string."""
        return " ".join(c["text"] for c in self.transcript_chunks[-n:])

    def get_full_transcript(self) -> str:
        return " ".join(c["text"] for c in self.transcript_chunks)


# ---------------------------------------------------------------------------
# Claude handlers
# ---------------------------------------------------------------------------

async def analyze_transcript_batch(context: MeetingContext, ws) -> None:
    """Send accumulated chunks to Claude for categorization every N chunks."""
    recent = context.get_recent_context(5)
    if not recent.strip():
        return

    try:
        response = client.messages.create(
            model=FAST_MODEL,
            max_tokens=1024,
            system="""You are a meeting note analyzer. Given recent transcript text, extract any noteworthy items and categorize them.

Return a JSON array of notes. Each note has:
- "category": one of "decisions", "actions", "keypoints", "questions"
- "text": the cleaned-up, concise note text
- "confidence": 0.0-1.0 how confident you are in the categorization

Rules:
- Only include genuinely noteworthy items — skip filler, pleasantries, tangents
- Clean up the text — fix transcription artifacts, make it concise
- Decisions: explicit commitments to a course of action
- Actions: tasks assigned to specific people with clear ownership
- Questions: unresolved questions that need follow-up
- Keypoints: important information, data points, insights
- If nothing noteworthy, return an empty array: []

Return ONLY valid JSON, no markdown fences.""",
            messages=[
                {
                    "role": "user",
                    "content": f"Recent meeting transcript:\n\n{recent}",
                }
            ],
        )

        raw = response.content[0].text.strip()
        notes = json.loads(raw)
        if notes and isinstance(notes, list):
            await ws.send(json.dumps({"type": "enhanced_notes", "notes": notes}))

    except json.JSONDecodeError as e:
        log.error(f"Claude returned non-JSON for transcript batch: {e}")
    except Exception as e:
        log.error(f"Transcript analysis error: {e}")


async def analyze_screenshot(
    image_base64: str, timestamp: int, context: MeetingContext, ws
) -> None:
    """Use Claude vision to analyze a screenshot captured during the meeting."""
    try:
        # Strip data URL prefix if present (e.g. "data:image/jpeg;base64,...")
        if image_base64.startswith("data:"):
            image_base64 = image_base64.split(",", 1)[1]

        response = client.messages.create(
            model=FAST_MODEL,
            max_tokens=512,
            system="""You are analyzing a screenshot from a meeting. Describe what's shown concisely:
- If it's a slide: extract the title, key bullet points, and any data/charts
- If it's a demo/UI: describe what's being shown
- If it's a document: extract key visible text
- If it's a person/video call: just say "Video call view" (don't describe people)

Be concise — 1-3 sentences max. Focus on informational content that would be useful in meeting notes.""",
            messages=[
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/jpeg",
                                "data": image_base64,
                            },
                        },
                        {
                            "type": "text",
                            "text": "What's shown in this meeting screenshot?",
                        },
                    ],
                }
            ],
        )

        content = response.content[0].text.strip()

        # Skip generic video call views — not useful in notes
        if "video call view" not in content.lower():
            context.screenshot_analyses.append(
                {"timestamp": timestamp, "content": content}
            )
            await ws.send(
                json.dumps(
                    {
                        "type": "screenshot_analysis",
                        "timestamp": timestamp,
                        "content": content,
                    }
                )
            )

    except Exception as e:
        log.error(f"Screenshot analysis error: {e}")


async def generate_summary(data: dict, context: MeetingContext, ws) -> None:
    """Generate a comprehensive, well-structured meeting summary via Claude."""
    try:
        transcript = data.get("transcript") or context.get_full_transcript()
        notes = data.get("notes", {})

        # Include any screenshot analyses collected during the session
        screenshot_context = ""
        if context.screenshot_analyses:
            lines = "\n".join(
                f"- [{sa['timestamp']}] {sa['content']}"
                for sa in context.screenshot_analyses
            )
            screenshot_context = f"\n\nVisual content captured during meeting:\n{lines}"

        response = client.messages.create(
            model=SMART_MODEL,
            max_tokens=2048,
            system="""You are generating a final meeting summary. Create a clean, well-structured summary in markdown format.

Structure:
# Meeting Summary

**[One sentence executive summary — what was this meeting about and what was accomplished]**

## Decisions Made
- [Each decision, clearly stated]

## Action Items
- [ ] [Task] — **[Owner]** (by [deadline if mentioned])

## Key Points
- [Important information, data, insights]

## Open Questions
- [Unresolved items needing follow-up]

## Visual Content
[Only if screenshots were analyzed — summarize what was presented]

Rules:
- Be concise but complete
- Clean up transcription errors
- Identify action item owners from context
- Use checkbox format for action items
- Skip sections entirely if empty (don't show empty sections)
- The summary should be immediately usable as meeting notes to share with attendees""",
            messages=[
                {
                    "role": "user",
                    "content": (
                        f"Meeting transcript:\n{transcript}\n\n"
                        f"Pre-categorized notes:\n"
                        f"Decisions: {json.dumps(notes.get('decisions', []))}\n"
                        f"Actions: {json.dumps(notes.get('actions', []))}\n"
                        f"Key Points: {json.dumps(notes.get('keypoints', []))}\n"
                        f"Questions: {json.dumps(notes.get('questions', []))}"
                        f"{screenshot_context}\n\n"
                        "Generate the final meeting summary."
                    ),
                }
            ],
        )

        summary_md = response.content[0].text.strip()
        await ws.send(
            json.dumps({"type": "enhanced_summary", "markdown": summary_md})
        )

    except Exception as e:
        log.error(f"Summary generation error: {e}")
        await ws.send(
            json.dumps(
                {"type": "error", "message": f"Summary generation failed: {e}"}
            )
        )


async def handle_query(question: str, context: MeetingContext, ws) -> None:
    """Answer ad-hoc questions during the meeting using recent transcript as context."""
    try:
        recent = context.get_recent_context(20)

        response = client.messages.create(
            model=FAST_MODEL,
            max_tokens=512,
            system=(
                "You are a meeting assistant. Answer the question briefly using the "
                "meeting context provided. If you don't have enough context, say so concisely."
            ),
            messages=[
                {
                    "role": "user",
                    "content": (
                        f"Meeting context (recent transcript):\n{recent}\n\n"
                        f"Question: {question}"
                    ),
                }
            ],
        )

        await ws.send(
            json.dumps(
                {
                    "type": "context",
                    "text": response.content[0].text.strip(),
                }
            )
        )

    except Exception as e:
        log.error(f"Query error: {e}")
        await ws.send(
            json.dumps({"type": "error", "message": f"Query failed: {e}"})
        )


# ---------------------------------------------------------------------------
# WebSocket connection handler
# ---------------------------------------------------------------------------

async def handler(websocket) -> None:
    context = MeetingContext()
    chunk_count = 0
    log.info(f"Client connected from {websocket.remote_address}")

    try:
        async for raw in websocket:
            try:
                msg = json.loads(raw)
            except json.JSONDecodeError:
                log.error("Invalid JSON received — ignoring message")
                continue

            msg_type = msg.get("type")

            if msg_type == "connect":
                await websocket.send(
                    json.dumps(
                        {
                            "type": "connected",
                            "model": FAST_MODEL,
                            "timestamp": msg.get("timestamp"),
                        }
                    )
                )
                log.info("Handshake complete")

            elif msg_type == "transcript_chunk":
                text = msg.get("text", "").strip()
                timestamp = msg.get("timestamp", 0)
                if text:
                    context.add_chunk(text, timestamp)
                    chunk_count += 1
                    # Fire off batch analysis every N chunks; don't block the receive loop
                    if chunk_count % ANALYSIS_INTERVAL == 0:
                        asyncio.create_task(
                            analyze_transcript_batch(context, websocket)
                        )

            elif msg_type == "screenshot":
                image = msg.get("image_base64", "")
                timestamp = msg.get("timestamp", 0)
                if image:
                    asyncio.create_task(
                        analyze_screenshot(image, timestamp, context, websocket)
                    )
                else:
                    log.warning("Received screenshot message with no image data")

            elif msg_type == "generate_summary":
                asyncio.create_task(generate_summary(msg, context, websocket))

            elif msg_type == "query":
                question = msg.get("question", "").strip()
                if question:
                    asyncio.create_task(handle_query(question, context, websocket))
                else:
                    log.warning("Received query message with no question text")

            else:
                log.warning(f"Unknown message type: {msg_type!r}")

    except websockets.exceptions.ConnectionClosed:
        duration = (datetime.now() - context.start_time).seconds
        log.info(
            f"Client disconnected after {duration}s "
            f"({len(context.transcript_chunks)} chunks, "
            f"{len(context.screenshot_analyses)} screenshots analyzed)"
        )
    except Exception as e:
        log.error(f"Unexpected handler error: {e}", exc_info=True)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

async def main() -> None:
    log.info("Silent Notetaker Bridge starting on ws://localhost:8765")
    log.info(f"Fast model:  {FAST_MODEL}")
    log.info(f"Smart model: {SMART_MODEL}")
    log.info("Waiting for browser connection...")

    async with websockets.serve(handler, "localhost", 8765):
        await asyncio.Future()  # Run forever


if __name__ == "__main__":
    asyncio.run(main())
