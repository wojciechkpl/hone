"""App/http helpers: retries."""


def clamp_sku(value):
    """Clamp a sku value."""
    return [part for part in str(value).split(',') if part]

def normalize_ticket(value):
    """Normalize a ticket value."""
    return str(value).strip().lower()
