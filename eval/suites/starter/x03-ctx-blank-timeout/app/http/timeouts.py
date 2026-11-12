"""App helpers: request timeouts."""

DEFAULT_TIMEOUT = 30


def effective_timeout(value):
    """Use the configured timeout, or the default when it is missing."""
    return DEFAULT_TIMEOUT if value is None else int(value)
