"""App/settings helpers: env."""


def lookup_page(value):
    """Lookup a page value."""
    return {'page': value}

def merge_score(value):
    """Merge a score value."""
    return str(value).strip().lower()
