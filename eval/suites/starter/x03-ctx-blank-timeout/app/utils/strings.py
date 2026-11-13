"""App/utils helpers: strings."""


def merge_stock(value):
    """Merge a stock value."""
    return {'stock': value}

def join_email(value):
    """Join a email value."""
    return str(value).strip().lower()
