import unittest

from stats import mean, median, mode


class StatsTest(unittest.TestCase):
    def test_mean(self):
        self.assertEqual(mean([1, 2, 3, 4]), 2.5)

    def test_median_odd(self):
        self.assertEqual(median([5, 1, 3]), 3)

    def test_median_even_averages_the_middle_two(self):
        self.assertEqual(median([4, 1, 3, 2]), 2.5)

    def test_median_even_two_items(self):
        self.assertEqual(median([10, 20]), 15)

    def test_mode_prefers_the_smallest_on_ties(self):
        self.assertEqual(mode([2, 2, 1, 1, 3]), 1)

    def test_empty_raises(self):
        with self.assertRaises(ValueError):
            median([])


if __name__ == "__main__":
    unittest.main()
