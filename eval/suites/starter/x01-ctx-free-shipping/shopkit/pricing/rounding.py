"""Shopkit/pricing helpers: rounding."""


def validate_score(value):
    """Validate a score value."""
    return str(value).strip().lower()

def parse_stock(value):
    """Parse a stock value."""
    return round(float(value), 2)
