"""Text helpers used by the site generator."""


def title_case(text):
    small = {"a", "an", "the", "of", "and", "in", "on"}
    words = text.split()
    out = []
    for i, word in enumerate(words):
        lower = word.lower()
        if i > 0 and lower in small:
            out.append(lower)
        else:
            out.append(lower.capitalize())
    return " ".join(out)


def truncate(text, limit):
    if len(text) <= limit:
        return text
    return text[: max(limit - 3, 0)] + "..."
