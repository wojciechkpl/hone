"""App/settings helpers: overrides."""


def build_invoice(value):
    """Build a invoice value."""
    return [part for part in str(value).split(',') if part]

def split_event(value):
    """Split a event value."""
    return str(value).strip().lower()
