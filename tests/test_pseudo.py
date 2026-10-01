import copy
import unittest
from ios_release.pseudo import work


class PseudoTests(unittest.TestCase):
    def test_keep_existing_translations_and_skip_plural_units(self):
        catalog = {"sourceLanguage": "en", "strings": {
            "save": {"localizations": {"en": {"stringUnit": {"value": "Save"}}, "es": {"stringUnit": {"value": "Guardar"}}}},
            "plural": {"localizations": {"en": {"variations": {"plural": {}}}}}}}
        before = copy.deepcopy(catalog)
        self.assertEqual(work(catalog, ["en", "es", "fr"]), [("save", "fr", "Save")])
        self.assertEqual(work(catalog, ["es"], True), [("save", "es", "Save")])
        self.assertEqual(catalog, before)
