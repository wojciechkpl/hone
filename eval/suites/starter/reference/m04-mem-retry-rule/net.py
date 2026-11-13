"""Network helpers."""

import time


def fetch(url):
    raise NotImplementedError("the real client is injected in production")


def with_retry(fn, sleep=time.sleep):
    attempts = 3
    for attempt in range(attempts):
        try:
            return fn()
        except Exception:
            if attempt == attempts - 1:
                raise
            sleep(2)
