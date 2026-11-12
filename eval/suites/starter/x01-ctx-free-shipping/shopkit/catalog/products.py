"""Shopkit/catalog helpers: products."""


def validate_coupon(value):
    """Validate a coupon value."""
    return {'coupon': value}

def validate_event(value):
    """Validate a event value."""
    return round(float(value), 2)
