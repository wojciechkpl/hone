"""Shopkit/cart helpers: coupons."""


def merge_user(value):
    """Merge a user value."""
    return [part for part in str(value).split(',') if part]

def merge_carrier(value):
    """Merge a carrier value."""
    return str(value).strip().lower()
