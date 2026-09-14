#!/bin/bash

# Compaction smoke test script
# Tests both manual (trigger prompt) and auto compaction (threshold-based)
#
# Environment variable overrides:
#   COMPACTION_PROVIDER - Override the provider for tests 1 & 2 (default: use system default)
#   COMPACTION_MODEL    - Override the model for tests 1 & 2 (default: use system default)
#   SKIP_BUILD          - Skip cargo build if set

if [ -f .env ]; then
  export $(grep -v '^#' .env | xargs)
fi

if [ -z "$SKIP_BUILD" ]; then
  echo "Building goose..."
  cargo build --bin goose
  echo ""
else
  echo "Skipping build (SKIP_BUILD is set)..."
  echo ""
fi

SCRIPT_DIR=$(pwd)
GOOSE_BIN="$SCRIPT_DIR/target/debug/goose"

# Apply provider/model overrides if set
if [ -n "$COMPACTION_PROVIDER" ]; then
  echo "Using override provider: $COMPACTION_PROVIDER"
  export GOOSE_PROVIDER="$COMPACTION_PROVIDER"
fi
if [ -n "$COMPACTION_MODEL" ]; then
  echo "Using override model: $COMPACTION_MODEL"
  export GOOSE_MODEL="$COMPACTION_MODEL"
fi
if [ -n "$COMPACTION_PROVIDER" ] || [ -n "$COMPACTION_MODEL" ]; then
  echo ""
fi

# Print the error proxy's request ledger. Without this, a TEST 3 failure gives
# no way to tell which upstream requests were made or which one received the
# injected error, leaving the request ordering to guesswork.
dump_proxy_log() {
  if [ -n "$PROXY_LOG" ] && [ -s "$PROXY_LOG" ]; then
    echo "   Proxy request log:"
    cat "$PROXY_LOG"
  fi
}

# Validation function to check compaction structure in session JSON
validate_compaction() {
  local session_id=$1
  local test_name=$2

  echo "Validating compaction structure for session: $session_id"

  # Export the session to JSON
  local session_json=$($GOOSE_BIN session export --format json --session-id "$session_id" 2>&1)

  if [ $? -ne 0 ]; then
    echo "✗ FAILED: Could not export session JSON"
    echo "   Error: $session_json"
    return 1
  fi

  if ! command -v jq &> /dev/null; then
    echo "⚠ WARNING: jq not available, cannot validate compaction structure"
    return 0
  fi

  # Check basic structure
  echo "$session_json" | jq -e '.conversation' > /dev/null 2>&1
  if [ $? -ne 0 ]; then
    echo "✗ FAILED: Session JSON missing 'conversation' field"
    return 1
  fi

  local message_count=$(echo "$session_json" | jq '.conversation | length' 2>/dev/null)
  echo "   Session has $message_count messages"

  # Look for a summary message (assistant role with userVisible=false, agentVisible=true)
  local has_summary=$(echo "$session_json" | jq '[.conversation[] | select(.role == "assistant" and .metadata.userVisible == false and .metadata.agentVisible == true)] | length > 0' 2>/dev/null)

  if [ "$has_summary" != "true" ]; then
    echo "✗ FAILED: No summary message found (expected assistant message with userVisible=false, agentVisible=true)"
    return 1
  fi
  echo "✓ Found summary message with correct visibility flags"

  # Check for original messages with userVisible=true, agentVisible=false
  local has_hidden_originals=$(echo "$session_json" | jq '[.conversation[] | select(.metadata.userVisible == true and .metadata.agentVisible == false)] | length > 0' 2>/dev/null)

  if [ "$has_hidden_originals" != "true" ]; then
    echo "⚠ WARNING: No original messages found with userVisible=true, agentVisible=false"
    echo "   This might be OK if all messages were compacted"
  else
    echo "✓ Found original messages hidden from agent (userVisible=true, agentVisible=false)"
  fi

  # For auto-compaction, check for the preserved user message (userVisible=true, agentVisible=true)
  local has_preserved_user=$(echo "$session_json" | jq '[.conversation[] | select(.role == "user" and .metadata.userVisible == true and .metadata.agentVisible == true)] | length > 0' 2>/dev/null)

  if [ "$has_preserved_user" == "true" ]; then
    echo "✓ Found preserved user message (userVisible=true, agentVisible=true)"
  fi

  echo "✓ SUCCESS: Compaction structure is valid for $test_name"
  return 0
}

echo "=================================================="
echo "COMPACTION SMOKE TESTS"
echo "=================================================="
echo ""

RESULTS=()

# ==================================================
# TEST 1: Manual Compaction
# ==================================================
echo "---------------------------------------------------"
echo "TEST 1: Manual Compaction via trigger prompt"
echo "---------------------------------------------------"

TESTDIR=$(mktemp -d)
echo "hello world" > "$TESTDIR/hello.txt"
echo "Test directory: $TESTDIR"
echo ""

OUTPUT=$(mktemp)

echo "Step 1: Creating session with initial messages..."
(cd "$TESTDIR" && "$GOOSE_BIN" run --with-builtin developer --text "list files and read hello.txt" 2>&1) | tee "$OUTPUT"

if ! command -v jq &> /dev/null; then
  echo "✗ FAILED: jq is required for this test"
  RESULTS+=("✗ Manual Compaction (jq required)")
  rm -f "$OUTPUT"
  rm -rf "$TESTDIR"
else
  SESSION_ID=$("$GOOSE_BIN" session list --format json 2>/dev/null | jq -r '.[0].id' 2>/dev/null)

  if [ -z "$SESSION_ID" ] || [ "$SESSION_ID" = "null" ]; then
    echo "✗ FAILED: Could not create session"
    RESULTS+=("✗ Manual Compaction (no session)")
  else
    echo ""
    echo "Session created: $SESSION_ID"
    echo "Step 2: Sending manual compaction trigger..."

    # Send the manual compact trigger prompt
    (cd "$TESTDIR" && "$GOOSE_BIN" run --resume --session-id "$SESSION_ID" --text "Please compact this conversation" 2>&1) | tee -a "$OUTPUT"

    echo ""
    echo "Checking for compaction evidence..."

    if grep -qi "compacting\|compacted\|compaction" "$OUTPUT"; then
      echo "✓ SUCCESS: Manual compaction was triggered"

      if validate_compaction "$SESSION_ID" "manual compaction"; then
        RESULTS+=("✓ Manual Compaction")
      else
        RESULTS+=("✗ Manual Compaction (structure validation failed)")
      fi
    else
      echo "✗ FAILED: Manual compaction was not triggered"
      RESULTS+=("✗ Manual Compaction")
    fi
  fi

  rm -f "$OUTPUT"
  rm -rf "$TESTDIR"
fi

echo ""
echo ""

# ==================================================
# TEST 2: Auto Compaction
# ==================================================
echo "---------------------------------------------------"
echo "TEST 2: Auto Compaction via threshold (0.005)"
echo "---------------------------------------------------"

TESTDIR=$(mktemp -d)
echo "test content" > "$TESTDIR/test.txt"
echo "Test directory: $TESTDIR"
echo ""

# Set auto-compact threshold very low (.5%) to trigger it quickly
export GOOSE_AUTO_COMPACT_THRESHOLD=0.005

OUTPUT=$(mktemp)

LONG_RESPONSE_PROMPT="Count from 1 to 200, one number per line."

echo "Step 1: Creating session with first message (generating tokens for threshold)..."
(cd "$TESTDIR" && "$GOOSE_BIN" run --text "$LONG_RESPONSE_PROMPT" 2>&1) | tee "$OUTPUT"

if ! command -v jq &> /dev/null; then
  echo "✗ FAILED: jq is required for this test"
  RESULTS+=("✗ Auto Compaction (jq required)")
else
  SESSION_ID=$("$GOOSE_BIN" session list --format json 2>/dev/null | jq -r '.[0].id' 2>/dev/null)

  if [ -z "$SESSION_ID" ] || [ "$SESSION_ID" = "null" ]; then
    echo "✗ FAILED: Could not create session"
    RESULTS+=("✗ Auto Compaction (no session)")
  else
    echo ""
    echo "Session created: $SESSION_ID"
    echo "Step 2: Sending second message (should trigger auto-compact)..."

    # Send second message - auto-compaction should trigger before processing this
    (cd "$TESTDIR" && "$GOOSE_BIN" run --resume --session-id "$SESSION_ID" --text "hi again" 2>&1) | tee -a "$OUTPUT"

    echo ""
    echo "Checking for auto-compaction evidence..."

    if grep -qi "auto.*compact\|exceeded.*auto.*compact.*threshold" "$OUTPUT"; then
      echo "✓ SUCCESS: Auto compaction was triggered"

      if validate_compaction "$SESSION_ID" "auto compaction"; then
        RESULTS+=("✓ Auto Compaction")
      else
        RESULTS+=("✗ Auto Compaction (structure validation failed)")
      fi
    else
      echo "✗ FAILED: Auto compaction was not triggered"
      echo "   Expected to see auto-compact messages with threshold of 0.005"
      RESULTS+=("✗ Auto Compaction")
    fi
  fi
fi

# Unset the env variable
unset GOOSE_AUTO_COMPACT_THRESHOLD

rm -f "$OUTPUT"
rm -rf "$TESTDIR"

echo ""
echo ""

# ==================================================
# TEST 3: Out-of-Context Error Compaction
# ==================================================
echo "---------------------------------------------------"
echo "TEST 3: Compaction via out-of-context error (proxy)"
echo "---------------------------------------------------"

TESTDIR=$(mktemp -d)
echo "test content" > "$TESTDIR/test.txt"
echo "Test directory: $TESTDIR"
echo ""

# Use a random port to avoid conflicts
PROXY_PORT=$((9000 + RANDOM % 1000))
PROXY_DIR="$SCRIPT_DIR/scripts/provider-error-proxy"

OUTPUT=$(mktemp)
PROXY_LOG=$(mktemp)
PROXY_SETUP_LOG=$(mktemp)

# Pre-install proxy dependencies (so first run doesn't take forever)
echo "Installing proxy dependencies..."
export UV_INDEX_URL="https://pypi.org/simple"
if ! (cd "$PROXY_DIR" && uv sync 2>&1 | tee "$PROXY_SETUP_LOG"); then
  echo "✗ FAILED: Could not install proxy dependencies"
  echo "Setup log:"
  cat "$PROXY_SETUP_LOG"
  RESULTS+=("✗ Out-of-Context Error (dependency install failed)")
else
  echo "✓ Dependencies installed"

  # Start the error proxy in context-length error mode.
  #
  # Inject exactly ONE error. The proxy only injects into completion requests
  # (see COMPLETION_PATHS in provider-error-proxy/proxy.py), so this error lands
  # on the turn completion and the summarizer call that follows succeeds,
  # producing the summary message this test validates.
  #
  # Do not raise this number to "give compaction more retries". A context-length
  # error is deterministic for a fixed prompt, so retrying an identical request
  # is not a scenario worth asserting; a higher count just consumes the
  # summarizer's own calls and prevents the summary this test exists to check.
  echo "Starting error proxy on port $PROXY_PORT with context-length error mode..."
  (cd "$PROXY_DIR" && UV_INDEX_URL="https://pypi.org/simple" uv run proxy.py --port "$PROXY_PORT" --mode "c 1" --no-stdin > "$PROXY_LOG" 2>&1) &
  PROXY_PID=$!

  # Wait for proxy to be ready (check if port is listening)
  echo "Waiting for proxy to be ready..."
  PROXY_READY=false
  for i in {1..60}; do
    if kill -0 $PROXY_PID 2>/dev/null; then
      # Check if port is listening using /dev/tcp
      if timeout 1 bash -c "echo -n > /dev/tcp/localhost/$PROXY_PORT" 2>/dev/null; then
        PROXY_READY=true
        echo "✓ Proxy is ready on port $PROXY_PORT"
        break
      fi
    else
      echo "✗ FAILED: Error proxy process died"
      break
    fi
    sleep 0.5
  done

  # Check if proxy is running and ready
  if [ "$PROXY_READY" != "true" ]; then
    echo "✗ FAILED: Error proxy failed to become ready"
    echo "Proxy log:"
    cat "$PROXY_LOG"
    kill $PROXY_PID 2>/dev/null || true
    RESULTS+=("✗ Out-of-Context Test Error (proxy failed)")
  else
    # Configure provider to use proxy and skip backoff
    export ANTHROPIC_HOST="http://localhost:$PROXY_PORT"
    export GOOSE_PROVIDER_SKIP_BACKOFF=true
    export GOOSE_PROVIDER=anthropic
    export GOOSE_MODEL=claude-haiku-4-5

    # Session naming runs as a background completion request spawned at the
    # start of the turn, so it races the turn completion for the proxy's single
    # injected error. When it wins, the naming call absorbs the error (it only
    # logs a warning), the turn succeeds, and no compaction ever happens.
    # Disable it so the turn completion is deterministically the first
    # completion request the proxy sees.
    export GOOSE_DISABLE_SESSION_NAMING=true

    echo "Step 1: Creating session (should trigger context-length error and compaction)..."
    (cd "$TESTDIR" && "$GOOSE_BIN" run --text "hello world" 2>&1) | tee "$OUTPUT"

    SESSION_ID=$("$GOOSE_BIN" session list --format json 2>/dev/null | jq -r '.[0].id' 2>/dev/null)

    if [ -z "$SESSION_ID" ] || [ "$SESSION_ID" = "null" ]; then
      echo "✗ FAILED: Could not create session"
      RESULTS+=("✗ Out-of-Context Test Error (no session)")
    else
      echo ""
      echo "Session created: $SESSION_ID"
      echo "Checking for compaction evidence..."

      # The compaction failure message itself contains the word "compact", so a
      # bare keyword grep matches it and reports success on a failed run. Fail
      # explicitly on the error text before checking for evidence of compaction.
      if grep -qi "error trying to compact" "$OUTPUT"; then
        echo "✗ FAILED: Compaction was attempted but returned an error"
        echo "   Output:"
        cat "$OUTPUT"
        dump_proxy_log
        RESULTS+=("✗ Out-of-Context Test Error (compaction errored)")
      elif grep -qi "context.*length\|compacting\|compacted\|compaction" "$OUTPUT"; then
        echo "✓ SUCCESS: Out-of-context Test error triggered compaction"

        if validate_compaction "$SESSION_ID" "out-of-context error compaction"; then
          RESULTS+=("✓ Out-of-Context Test Error")
        else
          dump_proxy_log
          RESULTS+=("✗ Out-of-Context Test Error (structure validation failed)")
        fi
      else
        echo "✗ FAILED: No evidence of compaction after context-length error"
        echo "   Output:"
        cat "$OUTPUT"
        dump_proxy_log
        RESULTS+=("✗ Out-of-Context Test Error")
      fi
    fi

    # Clean up
    echo ""
    echo "Stopping error proxy..."
    # Kill the entire process group to ensure UV and Python processes are terminated
    kill -- -$PROXY_PID 2>/dev/null || true
    # Also explicitly kill any remaining UV processes on this port
    pkill -f "uv run.*--port $PROXY_PORT" 2>/dev/null || true
    wait $PROXY_PID 2>/dev/null || true
    unset ANTHROPIC_HOST
    unset GOOSE_PROVIDER_SKIP_BACKOFF
    unset GOOSE_PROVIDER
    unset GOOSE_MODEL
    unset UV_INDEX_URL
  fi
fi

rm -f "$OUTPUT" "$PROXY_LOG" "$PROXY_SETUP_LOG"
rm -rf "$TESTDIR"

echo ""
echo ""

# ==================================================
# Summary
# ==================================================
echo "=================================================="
echo "TEST SUMMARY"
echo "=================================================="
for result in "${RESULTS[@]}"; do
  echo "$result"
done

# Count results
FAILURE_COUNT=$(echo "${RESULTS[@]}" | grep -o "✗" | wc -l | tr -d ' ')

if [ "$FAILURE_COUNT" -gt 0 ]; then
  echo ""
  echo "❌ $FAILURE_COUNT test(s) failed!"
  exit 1
else
  echo ""
  echo "✅ All tests passed!"
fi
