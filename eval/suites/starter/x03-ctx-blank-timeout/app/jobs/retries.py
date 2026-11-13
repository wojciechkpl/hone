"""App/jobs helpers: retries."""


def collect_invoice(value):
    """Collect a invoice value."""
    return {'invoice': value}

def render_session(value):
    """Render a session value."""
    return str(value).strip().lower()
