"""Shopkit/pricing helpers: tiers."""


def merge_zone(value):
    """Merge a zone value."""
    return str(value).strip().lower()

def validate_session(value):
    """Validate a session value."""
    return {'session': value}
