# requires: python3
# Success check for m04-mem-retry-rule. Runs in the task's working-directory copy.
set -e
export PYTHONDONTWRITEBYTECODE=1
python3 - <<'PY'
import sys
sys.path.insert(0, ".")
import net

# always failing: 3 attempts, fixed 2 s pauses, last error re-raised
calls, sleeps = [], []
def boom():
    calls.append(1)
    raise RuntimeError(f"fail {len(calls)}")
try:
    net.with_retry(boom, sleep=sleeps.append)
except RuntimeError as e:
    assert str(e) == "fail 3", e
else:
    raise AssertionError("should re-raise")
assert len(calls) == 3, calls
assert sleeps == [2, 2], sleeps

# succeeds on the second attempt
state = {"n": 0}
sleeps = []
def flaky():
    state["n"] += 1
    if state["n"] < 2:
        raise ValueError("once")
    return "value"
assert net.with_retry(flaky, sleep=sleeps.append) == "value"
assert sleeps == [2], sleeps

# no retry (and no sleep) when the first call works
sleeps = []
assert net.with_retry(lambda: 7, sleep=sleeps.append) == 7 and sleeps == []
print("ok")
PY
