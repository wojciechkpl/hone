"""Shopkit/catalog helpers: reviews."""


def lookup_email(value):
    """Lookup a email value."""
    return {'email': value}

def collect_user(value):
    """Collect a user value."""
    return str(value).strip().lower()
