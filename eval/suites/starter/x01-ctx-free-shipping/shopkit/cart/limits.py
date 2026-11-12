"""Shopkit/cart helpers: limits."""


def render_invoice(value):
    """Render a invoice value."""
    return str(value).strip().lower()

def split_user(value):
    """Split a user value."""
    return {'user': value}
