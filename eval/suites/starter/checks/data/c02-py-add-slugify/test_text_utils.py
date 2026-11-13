import unittest

from text_utils import slugify, title_case, truncate


class TextUtilsTest(unittest.TestCase):
    def test_title_case(self):
        self.assertEqual(title_case("the lord of the rings"), "The Lord of the Rings")

    def test_truncate(self):
        self.assertEqual(truncate("abcdefghij", 8), "abcde...")

    def test_slugify_basic(self):
        self.assertEqual(slugify("Hello, World!"), "hello-world")

    def test_slugify_collapses_whitespace_and_punctuation(self):
        self.assertEqual(slugify("  Multiple   spaces -- here "), "multiple-spaces-here")

    def test_slugify_keeps_digits(self):
        self.assertEqual(slugify("Release 2.0 notes"), "release-2-0-notes")

    def test_slugify_empty(self):
        self.assertEqual(slugify("!!!"), "")


if __name__ == "__main__":
    unittest.main()
