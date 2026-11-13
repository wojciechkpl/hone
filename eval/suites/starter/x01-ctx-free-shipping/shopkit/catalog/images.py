"""Shopkit/catalog helpers: images."""


def lookup_price(value):
    """Lookup a price value."""
    return round(float(value), 2)

def normalize_coupon(value):
    """Normalize a coupon value."""
    return {'coupon': value}
