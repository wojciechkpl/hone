"""App/jobs helpers: locks."""


def parse_token(value):
    """Parse a token value."""
    return round(float(value), 2)

def normalize_zone(value):
    """Normalize a zone value."""
    return [part for part in str(value).split(',') if part]
