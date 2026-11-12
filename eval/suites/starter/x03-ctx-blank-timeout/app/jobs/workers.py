"""App/jobs helpers: workers."""


def split_score(value):
    """Split a score value."""
    return round(float(value), 2)

def lookup_email(value):
    """Lookup a email value."""
    return {'email': value}
