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
# HOW IT CONNECTS TO CLAUDE
# -------------------------
# Default backend = the `claude` CLI (Claude Code) in headless print mode.
# If you are already logged into Claude Code (`claude` runs interactively for
# you), this bridge works with ZERO extra setup — it reuses your existing
# Claude subscription. No API key, no token pasting, nothing to configure.
#
#   How: each request shells out to `claude -p ... --output-format json` with
#   ANTHROPIC_API_KEY scrubbed from the child env. With the key scrubbed, the
#   CLI authenticates with your subscription's OAuth credentials (the ones it
#   already manages in your OS keychain) instead of a pay-per-use API key.
#
#   Why not call the API directly with the keychain OAuth token? Because the
#   Anthropic API rejects subscription OAuth tokens used outside of Claude Code
#   (HTTP 429 / rate_limit) unless you also forge the Claude Code system prompt
#   and beta headers. That's fragile and gray-area. Driving the real CLI is the
#   supported path and stays working across token refreshes.
#
# Fallback backend = direct Anthropic API with an explicit API key. Only used
# if the `claude` CLI is unavailable AND you set ANTHROPIC_API_KEY (or a saved
# token). This bills your API account, not your subscription.
#
# Run:    uv run bridge.py
# Port:   ws://localhost:8765
#
# Force a backend:   NOTETAKER_BACKEND=cli   (default, subscription)
#                    NOTETAKER_BACKEND=api   (explicit API key, billed)

import asyncio
import json
import os
import shutil
import subprocess
import tempfile
import logging
import sys
from pathlib import Path
from datetime import datetime

import websockets

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
)
log = logging.getLogger("bridge")

FAST_MODEL = "claude-sonnet-4-6"   # Real-time transcript analysis, screenshots, queries
SMART_MODEL = "claude-sonnet-4-6"  # Meeting summaries (swap to claude-opus-4-8 for production)

TOKEN_DIR = Path.home() / ".config" / "silent-notetaker"
TOKEN_FILE = TOKEN_DIR / "token"

ANALYSIS_INTERVAL = 5  # Analyze every N transcript chunks


# ---------------------------------------------------------------------------
# Claude backends
# ---------------------------------------------------------------------------
#
# Every backend exposes one async method:
#
#   async def complete(system, user_text, *, image_path=None, model) -> str
#
# returning the assistant's text. Handlers below are backend-agnostic.


class ClaudeCLIBackend:
    """Drive the `claude` CLI (Claude Code) in headless print mode.

    Reuses the user's existing Claude subscription. Zero extra auth.
    """

    name = "cli (Claude subscription)"

    def __init__(self, claude_bin: str):
        self.bin = claude_bin
        # Scrub ANTHROPIC_API_KEY so the CLI falls back to subscription OAuth
        # instead of a (possibly empty / billed) pay-per-use API key.
        self.env = {k: v for k, v in os.environ.items() if k != "ANTHROPIC_API_KEY"}
        # Run from a neutral cwd so we don't drag in a project's CLAUDE.md.
        self.cwd = str(Path(__file__).resolve().parent)

    async def complete(self, system: str, user_text: str, *,
                       image_path: str | None = None, model: str) -> str:
        cmd = [self.bin, "-p",
               "--model", model,
               "--output-format", "json",
               "--strict-mcp-config"]   # don't load any MCP servers
        if system:
            cmd += ["--system-prompt", system]

        prompt = user_text
        if image_path:
            # Vision: let the CLI read the image file via its Read tool.
            prompt = f"{user_text}\n\nThe image to analyze is at: {image_path}"
            cmd += ["--add-dir", os.path.dirname(image_path),
                    "--allowedTools", "Read"]

        proc = await asyncio.create_subprocess_exec(
            *cmd,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=self.env,
            cwd=self.cwd,
        )
        # Pipe the prompt via stdin — argv has length limits, transcripts don't.
        stdout, stderr = await proc.communicate(input=prompt.encode("utf-8"))

        if proc.returncode != 0:
            raise RuntimeError(
                f"claude CLI exited {proc.returncode}: {stderr.decode('utf-8', 'replace')[:300]}"
            )

        outer = json.loads(stdout.decode("utf-8"))
        if outer.get("is_error"):
            raise RuntimeError(f"claude CLI error: {outer.get('result')!r}")
        return (outer.get("result") or "").strip()


class AnthropicAPIBackend:
    """Direct Anthropic API with an explicit API key. Bills the API account."""

    name = "api (explicit API key — billed)"

    def __init__(self, api_key: str):
        import anthropic  # imported lazily so the CLI path needs no SDK
        self.client = anthropic.Anthropic(api_key=api_key)

    async def complete(self, system: str, user_text: str, *,
                       image_path: str | None = None, model: str) -> str:
        if image_path:
            import base64
            data = base64.b64encode(Path(image_path).read_bytes()).decode()
            content = [
                {"type": "image", "source": {"type": "base64",
                                             "media_type": "image/jpeg", "data": data}},
                {"type": "text", "text": user_text},
            ]
        else:
            content = user_text

        # SDK is sync — run it off the event loop.
        def _call():
            resp = self.client.messages.create(
                model=model, max_tokens=2048, system=system,
                messages=[{"role": "user", "content": content}],
            )
            return resp.content[0].text.strip()

        return await asyncio.to_thread(_call)


def get_saved_token() -> str | None:
    if TOKEN_FILE.exists():
        token = TOKEN_FILE.read_text().strip()
        if token:
            return token
    return None


def save_token(token: str) -> None:
    TOKEN_DIR.mkdir(parents=True, exist_ok=True)
    TOKEN_FILE.write_text(token)
    TOKEN_FILE.chmod(0o600)
    log.info(f"Auth: API key saved to {TOKEN_FILE}")


def cli_is_usable(claude_bin: str) -> bool:
    """Confirm the CLI can actually authenticate (logged in, not just present)."""
    env = {k: v for k, v in os.environ.items() if k != "ANTHROPIC_API_KEY"}
    try:
        proc = subprocess.run(
            [claude_bin, "-p", "--model", FAST_MODEL, "--output-format", "json",
             "--strict-mcp-config"],
            input="Reply with exactly OK", capture_output=True, text=True,
            env=env, timeout=60,
        )
        if proc.returncode != 0:
            log.warning(f"CLI probe failed (exit {proc.returncode}): {proc.stderr[:200]}")
            return False
        d = json.loads(proc.stdout)
        if d.get("is_error"):
            log.warning(f"CLI probe returned error: {d.get('result')!r}")
            return False
        return True
    except Exception as e:
        log.warning(f"CLI probe exception: {e}")
        return False


def select_backend():
    """Pick the lowest-friction working backend.

    Order: CLI (subscription, zero setup) → explicit API key (billed).
    """
    forced = os.environ.get("NOTETAKER_BACKEND", "").lower()

    claude_bin = shutil.which("claude")

    if forced != "api" and claude_bin:
        log.info(f"Probing Claude CLI at {claude_bin} ...")
        if cli_is_usable(claude_bin):
            log.info("Auth: Using your Claude subscription via the `claude` CLI (no API key needed).")
            return ClaudeCLIBackend(claude_bin)
        log.warning("Claude CLI found but not authenticated. Run `claude` once and log in.")

    # Fallback: explicit API key (billed to your API account).
    api_key = os.environ.get("ANTHROPIC_API_KEY") or get_saved_token()
    if api_key:
        log.info("Auth: Using explicit ANTHROPIC_API_KEY / saved token (billed to API account).")
        return AnthropicAPIBackend(api_key)

    # Nothing works — guide the user to the zero-cost subscription path.
    log.error("No working Claude connection found.")
    log.error("Easiest fix (uses your subscription, no API key):")
    log.error("   1. Install Claude Code:  https://claude.com/claude-code")
    log.error("   2. Run `claude` once and log in.")
    log.error("   3. Restart this bridge — it will detect the CLI automatically.")
    log.error("Alternative: set ANTHROPIC_API_KEY (this bills your API account).")
    sys.exit(1)


backend = select_backend()


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
        return " ".join(c["text"] for c in self.transcript_chunks[-n:])

    def get_full_transcript(self) -> str:
        return " ".join(c["text"] for c in self.transcript_chunks)


def _strip_json_fences(text: str) -> str:
    """Defensive: drop ```json fences if a model wraps its output."""
    t = text.strip()
    if t.startswith("```"):
        t = t.split("\n", 1)[1] if "\n" in t else t[3:]
        if t.rstrip().endswith("```"):
            t = t.rstrip()[:-3]
    return t.strip()


# ---------------------------------------------------------------------------
# Claude handlers (backend-agnostic)
# ---------------------------------------------------------------------------

async def analyze_transcript_batch(context: MeetingContext, ws) -> None:
    """Send accumulated chunks to Claude for categorization every N chunks."""
    recent = context.get_recent_context(5)
    if not recent.strip():
        return

    try:
        raw = await backend.complete(
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
            user_text=f"Recent meeting transcript:\n\n{recent}",
            model=FAST_MODEL,
        )

        notes = json.loads(_strip_json_fences(raw))
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
    tmp_path = None
    try:
        # Strip data URL prefix if present (e.g. "data:image/jpeg;base64,...")
        if image_base64.startswith("data:"):
            image_base64 = image_base64.split(",", 1)[1]

        import base64
        img_bytes = base64.b64decode(image_base64)
        fd, tmp_path = tempfile.mkstemp(suffix=".jpg", prefix="notetaker_shot_")
        with os.fdopen(fd, "wb") as f:
            f.write(img_bytes)

        content = await backend.complete(
            system="""You are analyzing a screenshot from a meeting. Describe what's shown concisely:
- If it's a slide: extract the title, key bullet points, and any data/charts
- If it's a demo/UI: describe what's being shown
- If it's a document: extract key visible text
- If it's a person/video call: just say "Video call view" (don't describe people)

Be concise — 1-3 sentences max. Focus on informational content that would be useful in meeting notes.""",
            user_text="What's shown in this meeting screenshot?",
            image_path=tmp_path,
            model=FAST_MODEL,
        )

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
    finally:
        if tmp_path and os.path.exists(tmp_path):
            try:
                os.unlink(tmp_path)
            except OSError:
                pass


async def generate_summary(data: dict, context: MeetingContext, ws) -> None:
    """Generate a comprehensive, well-structured meeting summary via Claude."""
    try:
        transcript = data.get("transcript") or context.get_full_transcript()
        notes = data.get("notes", {})

        screenshot_context = ""
        if context.screenshot_analyses:
            lines = "\n".join(
                f"- [{sa['timestamp']}] {sa['content']}"
                for sa in context.screenshot_analyses
            )
            screenshot_context = f"\n\nVisual content captured during meeting:\n{lines}"

        summary_md = await backend.complete(
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
            user_text=(
                f"Meeting transcript:\n{transcript}\n\n"
                f"Pre-categorized notes:\n"
                f"Decisions: {json.dumps(notes.get('decisions', []))}\n"
                f"Actions: {json.dumps(notes.get('actions', []))}\n"
                f"Key Points: {json.dumps(notes.get('keypoints', []))}\n"
                f"Questions: {json.dumps(notes.get('questions', []))}"
                f"{screenshot_context}\n\n"
                "Generate the final meeting summary."
            ),
            model=SMART_MODEL,
        )

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

        answer = await backend.complete(
            system=(
                "You are a meeting assistant. Answer the question briefly using the "
                "meeting context provided. If you don't have enough context, say so concisely."
            ),
            user_text=(
                f"Meeting context (recent transcript):\n{recent}\n\n"
                f"Question: {question}"
            ),
            model=FAST_MODEL,
        )

        await ws.send(json.dumps({"type": "context", "text": answer}))

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
    log.info(f"Backend:     {backend.name}")
    log.info(f"Fast model:  {FAST_MODEL}")
    log.info(f"Smart model: {SMART_MODEL}")
    log.info("Waiting for browser connection...")

    async with websockets.serve(handler, "localhost", 8765):
        await asyncio.Future()  # Run forever


if __name__ == "__main__":
    asyncio.run(main())
