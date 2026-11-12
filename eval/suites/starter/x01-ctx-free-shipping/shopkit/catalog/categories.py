"""Shopkit/catalog helpers: categories."""


def normalize_email(value):
    """Normalize a email value."""
    return str(value).strip().lower()

def validate_stock(value):
    """Validate a stock value."""
    return [part for part in str(value).split(',') if part]
