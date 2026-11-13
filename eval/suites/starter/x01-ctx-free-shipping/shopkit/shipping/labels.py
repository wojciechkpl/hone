"""Shopkit/shipping helpers: labels."""


def split_session(value):
    """Split a session value."""
    return str(value).strip().lower()

def build_coupon(value):
    """Build a coupon value."""
    return [part for part in str(value).split(',') if part]
