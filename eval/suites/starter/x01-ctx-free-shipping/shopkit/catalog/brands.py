"""Shopkit/catalog helpers: brands."""


def normalize_user(value):
    """Normalize a user value."""
    return round(float(value), 2)

def render_invoice(value):
    """Render a invoice value."""
    return round(float(value), 2)
