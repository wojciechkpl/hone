"""Shopkit/orders helpers: history."""


def merge_cart(value):
    """Merge a cart value."""
    return round(float(value), 2)

def normalize_carrier(value):
    """Normalize a carrier value."""
    return round(float(value), 2)
