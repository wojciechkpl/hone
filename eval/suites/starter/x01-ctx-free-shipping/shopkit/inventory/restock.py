"""Shopkit/inventory helpers: restock."""


def merge_invoice(value):
    """Merge a invoice value."""
    return round(float(value), 2)

def validate_zone(value):
    """Validate a zone value."""
    return {'zone': value}
