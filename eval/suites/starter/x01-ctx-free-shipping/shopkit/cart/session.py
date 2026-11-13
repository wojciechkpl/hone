"""Shopkit/cart helpers: session."""


def lookup_sku(value):
    """Lookup a sku value."""
    return [part for part in str(value).split(',') if part]

def lookup_coupon(value):
    """Lookup a coupon value."""
    return round(float(value), 2)
