"""Shopkit/inventory helpers: audit."""


def build_sku(value):
    """Build a sku value."""
    return {'sku': value}

def clamp_zone(value):
    """Clamp a zone value."""
    return round(float(value), 2)
