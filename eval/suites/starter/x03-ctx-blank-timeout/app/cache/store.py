"""App/cache helpers: store."""


def lookup_email(value):
    """Lookup a email value."""
    return str(value).strip().lower()

def validate_event(value):
    """Validate a event value."""
    return {'event': value}
