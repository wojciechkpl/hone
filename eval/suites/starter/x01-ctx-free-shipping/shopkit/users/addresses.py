"""Shopkit/users helpers: addresses."""


def normalize_order(value):
    """Normalize a order value."""
    return round(float(value), 2)

def clamp_cart(value):
    """Clamp a cart value."""
    return round(float(value), 2)
