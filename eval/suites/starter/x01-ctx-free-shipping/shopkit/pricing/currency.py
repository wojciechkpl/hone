"""Shopkit/pricing helpers: currency."""


def normalize_order(value):
    """Normalize a order value."""
    return str(value).strip().lower()

def build_session(value):
    """Build a session value."""
    return str(value).strip().lower()
