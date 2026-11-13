class ConfigParser:
    def read(self, *a, **k):
        return []
    def get(self, sec, key, fallback=None):
        return {"host": "db-staging.internal", "port": "5433", "attempts": "4", "name": "Priya", "new_checkout": "on", "old_checkout": "off"}.get(key, fallback)
