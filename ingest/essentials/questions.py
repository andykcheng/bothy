"""Classification questions (Jev / SystemOne schema) for the essentials tier, versioned. Shared by
jev_classify.py (hosted Jev) and clef_classify.py (local Clef). v5 is the default."""
QUESTIONS = {
 "v1": {"useful": {"type": "score",
    "instructions": "How useful would this encyclopedia article be in a small (1 GB) offline library for people in the "
                    "United Kingdom during a long emergency with no internet, no shops and maybe no doctor?",
    "criteria": [
        "Not useful: entertainment, sport, biographies, politics, places, organisations, history of events, or obscure specialist detail",
        "Background knowledge only; rarely needed in practice",
        "Useful practical or reference knowledge (health, food, water, shelter, energy, tools, repair, plants, animals, basic science)",
        "Essential: directly helps people stay alive, healthy, fed, warm or safe"]}},
 # v2 (after auditing 300 of the top-20k): judge practical value in Britain, not general importance; push down
 # foreign dishes and non-British species, individual prescription drugs, hospital-only medicine, academic theory,
 # and articles about books, databases or phrases.
 "v2": {"useful": {"type": "score",
    "instructions": "Judge the practical value of this encyclopedia article to ordinary people in Britain during a long "
                    "emergency with no internet, no shops, no electricity grid and maybe no doctor, who keep only a small "
                    "(1 GB) offline library. Judge what a reader could actually do with it there, not how important "
                    "or well known the topic is.",
    "criteria": [
        "Not useful: entertainment, sport, people, politics, law, places, organisations, companies, events and disasters, "
        "books, journals, databases, websites, phrases; dishes, crops, plants or animals of other countries that are "
        "not found in Britain",
        "Background only: general or academic science and theory, anatomy detail, rare diseases, hospital-only "
        "procedures and specialist drugs, history of a subject, technology that needs factories or the grid",
        "Useful in practice: common illnesses and injuries and their care, common medicines, food growing, foraging, "
        "cooking and preserving with basic means, water, sanitation and hygiene, heating, shelter, clothing, tools, "
        "repair, crafts and materials, livestock, British wild plants, fungi and animals, weather, navigation and signalling",
        "Essential: directly keeps people alive, healthy, fed, warm or safe (first aid, dangerous poisonous species, "
        "safe water, fire, shelter, outbreaks of infectious disease)"]}},
 # v3: v1's wording plus only the targeted fixes from the audit (v2 over-reached: it raised computing and military
 # hardware into "background" and pushed down useful health and heating articles).
 "v3": {"useful": {"type": "score",
    "instructions": "How useful would this encyclopedia article be in a small (1 GB) offline library for people in "
                    "Britain during a long emergency with no internet, no shops and maybe no doctor?",
    "criteria": [
        "Not useful: entertainment, sport, biographies, politics, places, organisations, history of events, books, "
        "journals, databases, obscure specialist detail, or dishes, crops, plants and animals of other countries that "
        "are not found in Britain",
        "Background knowledge only, rarely needed in practice: general science and theory, rare diseases, hospital-only "
        "treatments and individual prescription drugs",
        "Useful practical or reference knowledge (health, food, water, shelter, energy, tools, repair, materials, "
        "signalling, British plants, fungi and animals, basic science)",
        "Essential: directly helps people stay alive, healthy, fed, warm or safe"]}},
 # v4: region-neutral (the app is for people anywhere). v3's non-geographic fixes kept: books/databases are not useful;
 # theory, rare diseases, hospital-only treatments and individual prescription drugs are background.
 "v4": {"useful": {"type": "score",
    "instructions": "How useful would this encyclopedia article be in a small (1 GB) offline library for people "
                    "anywhere in the world during a long emergency with no internet, no shops and maybe no doctor?",
    "criteria": [
        "Not useful: entertainment, sport, biographies, politics, places, organisations, history of events, books, "
        "journals, databases, or obscure specialist detail",
        "Background knowledge only, rarely needed in practice: general science and theory, rare diseases, hospital-only "
        "treatments and individual prescription drugs",
        "Useful practical or reference knowledge (health, food, water, shelter, energy, tools, repair, materials, "
        "signalling, plants, fungi and animals, basic science)",
        "Essential: directly helps people stay alive, healthy, fed, warm or safe"]}},
 # v5 (default): v4, but common and essential medicines (antibiotics, painkillers, insulin) count as useful;
 # v4 had pushed doxycycline, penicillin and insulin down to ~1.2. Specialist drugs stay background.
 "v5": {"useful": {"type": "score",
    "instructions": "How useful would this encyclopedia article be in a small (1 GB) offline library for people "
                    "anywhere in the world during a long emergency with no internet, no shops and maybe no doctor?",
    "criteria": [
        "Not useful: entertainment, sport, biographies, politics, places, organisations, history of events, books, "
        "journals, databases, or obscure specialist detail",
        "Background knowledge only, rarely needed in practice: general science and theory, rare diseases, hospital-only "
        "treatments and specialist drugs",
        "Useful practical or reference knowledge (health, common and essential medicines such as antibiotics, painkillers and insulin, food, water, shelter, energy, tools, repair, materials, "
        "signalling, plants, fungi and animals, basic science)",
        "Essential: directly helps people stay alive, healthy, fed, warm or safe"]}},
}

# Public-domain books (Project Gutenberg) for the "Practical books" collection: scored from catalogue data
# (title, author, subjects, summary), not the full text. Old medicine is out (WikiMed covers it, and
# 19th-century treatments are often harmful).
QUESTIONS["books-practical"] = {"useful": {"type": "score",
    "instructions": "This is a public-domain book (usually 1850-1930). How useful would it be as a practical manual in an "
                    "offline library for people anywhere in the world living without the internet, shops or modern supply "
                    "chains for a long time?",
    "criteria": [
        "Not useful: fiction, poetry, history, biography, religion, etiquette, business or money-making, games and sport, "
        "art history, periodicals, or medical and veterinary treatment advice (outdated and possibly harmful)",
        "Background only: general science, natural history descriptions, travel, or skills rarely needed",
        "Useful practical know-how: growing food, keeping animals, cooking and preserving, crafts (wood, metal, textiles, "
        "leather, pottery, soap), building and repair, tools and simple machines, outdoor living, knots, navigation, "
        "identifying wild plants, fungi and animals",
        "Essential: directly helps people produce food, water, shelter, warmth or tools by themselves",
    ]}}
