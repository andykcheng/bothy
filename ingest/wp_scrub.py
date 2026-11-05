"""Remove Wikipedia's editor-facing text that survives the class-based cleaning in zim_extract.py: notes asking
readers to edit the page and inline "citation needed" tags (readers of an offline copy can't act on them), and the
periodic-table navigation box in element infoboxes.

  scrub_html(html)  for a doc's html
  scrub_text(text)  for block and chunk text (same rules, on plain text)
  scrub_doc(d)      both, on a docs.jsonl record (html and blocks)

Used by zim_extract.py (new extracts) and essentials/subset.py (packs cut from older extracts).
"""
import re

# Electronic-component infobox: "⚠ Please help improve Wikipedia ⚠ by adding the alt parameter. ... Thanks ♥"
_ALT_HTML = re.compile(r"⚠\s*Please help improve Wikipedia\s*⚠.*?Thanks\s*(?:</i>)?\s*[♥♡]\s*(?:<br>\s*)*", re.S)
_ALT_TEXT = re.compile(r"⚠\s*Please help improve Wikipedia\s*⚠.*?Thanks\s*[♥♡]\n?", re.S)
# Inline tags: [citation needed], [medical citation needed], {citation needed}, {{citation needed|date=...}}
_CN_HTML = re.compile(r"\s?(?:<sup>\s*)?(?:\[|\{\{?|\()\s*(?:<i>)?\s*(?:medical )?citation needed(?:\|[^}\]]*)?\s*(?:</i>)?"
                      r"\s*(?:\]|\}\}?|\))(?:\s*</sup>)?", re.I)
_CN_TEXT = re.compile(r"\s?(?:\[|\{\{?|\()\s*(?:medical )?citation needed(?:\|[^}\]]*)?\s*(?:\]|\}\}?|\))", re.I)
# Requests to readers inside list and glossary intros (the rest of the sentence stays).
_HELP = re.compile(r"\s*(?:This is a dynamic list and may never be able to satisfy particular standards for "
                   r"completeness\.\s*)?You can help (?:by adding missing items with reliable sources|enhance this page "
                   r"by adding new terms or writing definitions for existing ones)\.?"
                   r"|\s*(?:<i>)?This (?:part|section|list) is not finished, you can help by expanding it\.(?:</i>)?")
# Chemical-element infobox: "X in the periodic table" with an 18-column table of every element. It is only links,
# and it makes the infobox several screens wide, pushing the rest of it off the side of a phone screen.
_PTABLE = re.compile(r'<tr><th colspan="2">[^<]{0,80} in the <a href="wiki:Periodic_table">periodic table</a></th></tr>'
                     r'<tr><td colspan="2">\s*<table>.*?</table>.*?</td></tr>', re.S)
_EMPTY_P = re.compile(r'<p id="(p\d+)">\s*</p>\s*')


def scrub_text(s: str) -> str:
    if not s:
        return s
    s = _ALT_TEXT.sub("", s)
    s = _CN_TEXT.sub("", s)
    return _HELP.sub("", s)


def scrub_html(h: str) -> str:
    if not h:
        return h
    empty_before = set(_EMPTY_P.findall(h))  # keep paragraphs that were empty in the source (blocks point at them)
    h = _ALT_HTML.sub("", h)
    h = _PTABLE.sub("", h)
    h = _CN_HTML.sub("", h)
    h = _HELP.sub("", h)
    return _EMPTY_P.sub(lambda m: m.group(0) if m.group(1) in empty_before else "", h)


def scrub_doc(d: dict) -> dict:
    d["html"] = scrub_html(d["html"])
    blocks = []
    for b in d.get("blocks") or []:
        t = scrub_text(b["text"])
        if b["text"] and not t.strip():  # held only a removed note; its <p> is gone from the html too
            continue
        b["text"] = t
        blocks.append(b)
    d["blocks"] = blocks
    return d
