import json, sys
EN = {
'Alles auf diesem iPhone bleibt, es wird nur nicht mehr abgeglichen.':"Everything on this iPhone stays; it just isn't synced anymore.",
'Anderer Bereich':'Different space',
'Angemeldet':'Signed in',
'Anmelden':'Sign in',
'Auf dem anderen Gerät „Mit Konto verbinden“ wählen und die Wörter eintippen oder den Code einfügen. Der Code gilt fünf Minuten.':'On the other device, choose “Join account” and type the words or paste the code. The code is valid for five minutes.',
'Auf einem Gerät, das schon verbunden ist: „Weiteres Gerät hinzufügen“.':"On a device that's already connected: “Add another device”.",
'Aufgeschrieben':'Written down',
'Ausstehend':'Pending',
'Authenticator-App':'Authenticator app',
'Code':'Code',
'Code ist unterwegs':'Code is on its way',
'Code kopiert':'Code copied',
'Code per E-Mail senden':'Send code by email',
'Code vom anderen Gerät':'Code from the other device',
'Code zum Einfügen kopieren':'Copy code for pasting',
'Das Konto hat einen anderen UwUNews-Bereich als bisher. Auf dieses iPhone kommt dann, was dort liegt; was hier ist, geht mit hoch.':"The account has a different UwUNews space than before. This iPhone gets what's there; what's here is uploaded too.",
'Den Einrichtungscode zeigt der UwUSync-Server beim ersten Start oder per Admin-Befehl.':'The UwUSync server shows the setup code on first start or via an admin command.',
'Der Schlüssel fehlt auf diesem Gerät. Mit dem Master-Passwort kommt er zurück.':'The key is missing on this device. The master password brings it back.',
'Der eingefügte Code enthält den Server schon.':'A pasted code already includes the server.',
'Dienst':'Service',
'Diesen Bereich nehmen':'Use this space',
'Dieses Gerät merken':'Remember this device',
'E-Mail':'Email',
'Ein eigenes UwUSync-Konto, wie bei UwUSSH und UwURDP. Der Server sieht nur verschlüsselte Daten.':'Your own UwUSync account, as with UwUSSH and UwURDP. The server only sees encrypted data.',
'Einrichtungscode vom Server':'Setup code from the server',
'Entsperren':'Unlock',
'Fingerabdruck (optional)':'Fingerprint (optional)',
'Geräte':'Devices',
'Gesperrt':'Locked',
'Jetzt abgleichen':'Sync now',
'Kein passender zweiter Faktor':'No usable second factor',
'Konto':'Account',
'Konto anlegen':'Create account',
'Konto von':'Account from',
'Kopieren':'Copy',
'Master-Passwort':'Master password',
'Master-Passwort wiederholen':'Repeat master password',
'Methode':'Method',
'Mit deinem UwULock-Konto: UwUNews bekommt dort einen eigenen Bereich.':'With your UwULock account: UwUNews gets its own space there.',
'Neues':'New',
'Noch nie':'Never',
'Nur bei gesprochenem Code':'Only for a spoken code',
'Passkey':'Passkey',
'Richte in UwULock eine Authenticator-App oder E-Mail-Codes ein.':'Set up an authenticator app or email codes in UwULock.',
'Schreib ihn auf und bewahr ihn sicher auf. Ohne ihn und ohne Master-Passwort sind die Daten verloren.':'Write it down and keep it safe. Without it and the master password, the data is lost.',
'Server':'Server',
'Server-Adresse':'Server address',
'Sync beenden':'Stop syncing',
'Sync beenden?':'Stop syncing?',
'Verbinden':'Connect',
'Verbunden':'Connected',
'Vorhandenes':'Existing',
'Warte auf das andere Gerät …':'Waiting for the other device …',
'Weiteres Gerät hinzufügen':'Add another device',
'Wiederherstellungscode':'Recovery code',
'Zuletzt abgeglichen':'Last synced',
'Zweiter Faktor':'Second factor',
'%@ ist verbunden':'%@ is connected',
"1 Monat":"1 month","2 Monate":"2 months","2 Wochen":"2 weeks","6 Monate":"6 months",
"Abbrechen":"Cancel","Abruf fehlgeschlagen":"Couldn't fetch","Adresse":"Address","Alle Artikel":"All articles",
"Alle Quellen":"All sources","Alle als gelesen markieren":"Mark all as read","Alle davon":"All of these",
"Alles gelesen":"All caught up","Als gelesen markieren":"Mark as read","Als ungelesen markieren":"Mark as unread",
"Anlegen":"Create","Artikel durchsuchen":"Search articles","Bearbeiten":"Edit",
"Bei neuen Artikeln benachrichtigen":"Notify about new articles","Benachrichtigen":"Notify","Benachrichtigungen an":"Notifications on",
"Das hat nicht geklappt":"That didn't work","Der Speicher der App ließ sich nicht öffnen.":"The app's storage couldn't be opened.",
"Die Quellen und ihre Artikel bleiben.":"The sources and their articles stay.",
"Eine Sammlung fasst Quellen zusammen. Mit Filterwörtern zeigt sie nur, was passt.":"A collection groups sources. With filter words it only shows what matches.",
"Einstellungen":"Settings","Entfernen":"Remove","Erste Quelle hinzufügen":"Add your first source","Feed":"Feed","Feeds":"Feeds",
"Fehler: %@":"Error: %@","Filter-Feed":"Filter feed",
"Füge eine Webseite oder die Adresse eines RSS- oder Atom-Feeds hinzu.":"Add a website or the address of an RSS or Atom feed.",
"Gefunden":"Found","Gelesen":"Read","Gelesene Artikel behalten":"Keep read articles","Gemerkt":"Saved",
"Größere Schrift":"Larger text","Heute":"Today","Hinzufügen":"Add",
"Ihre Artikel verschwinden, gemerkte bleiben erhalten.":"Its articles go away; saved ones stay.",
"Im Browser öffnen":"Open in browser","In Sammlungen":"In collections","Keine Verbindung: %@":"No connection: %@",
"Keins davon":"None of these","Kleinere Schrift":"Smaller text","Leer lassen, wenn es egal ist.":"Leave empty if it doesn't matter.",
"Lesen":"Reading","Links in der App öffnen":"Open links in the app","Löschen":"Delete",
"Meldet jeden neuen Artikel, der in diese Sammlung fällt, auch aus Quellen ohne Glocke.":"Announces every new article that lands in this collection, even from sources without a bell.",
"Merken":"Save","Mindestens eins davon":"At least one of these","Mitteilungen":"Notifications","Mitteilungen erlauben":"Allow notifications",
"Mitteilungen erlaubt":"Notifications allowed","Mitteilungen in den Einstellungen erlauben":"Allow notifications in Settings",
"Nach Wörtern filtern":"Filter by words","Name":"Name",
"Neue Artikel erscheinen hier, sobald sie veröffentlicht werden.":"New articles show up here as soon as they're published.",
"Neue Artikel werden jetzt geladen":"Loading new articles now","Neue Sammlung":"New collection","Neuer Filter-Feed":"New filter feed",
"Nicht mehr merken":"Unsave","Nichts gemerkt":"Nothing saved","Noch keine Artikel":"No articles yet","Noch keine Feeds":"No feeds yet",
"Nur ungelesene":"Unread only","Nächster Artikel":"Next article","OPML exportieren":"Export OPML","OPML importieren":"Import OPML",
"OPML ist die Liste, die fast jeder Feed-Reader lesen und schreiben kann.":"OPML is the list almost every feed reader can read and write.",
"Ohne Filter zeigt die Sammlung alles aus ihren Quellen.":"Without a filter the collection shows everything from its sources.",
"Quellcode (GPL-3.0)":"Source code (GPL-3.0)","Quelle":"Source","Quelle entfernen":"Remove source","Quelle hinzufügen":"Add source","Quellen":"Sources",
"Quellen, Sammlungen und was gelesen oder gemerkt ist, Ende-zu-Ende-verschlüsselt über UwUSync oder UwULock.":"Sources, collections and what's read or saved, end-to-end encrypted via UwUSync or UwULock.",
"Sammlung":"Collection","Sammlung löschen":"Delete collection","Sammlungen":"Collections",
"Sammlungen mit eigener Glocke melden ihre Artikel auch, wenn sie hier aus ist.":"Collections with their own bell still announce its articles when this is off.",
"Sichern":"Save","Speicherfehler: %@":"Storage error: %@","Stumm":"Mute","Suche":"Search","Suche nach einem Feed …":"Looking for a feed …",
"Suchen":"Search","Suchen in":"Search in","Sync":"Sync","Teilen":"Share","Titel":"Title","Titel und Text":"Title and text",
"Umziehen":"Move","Ungelesen":"Unread","Unter dieser Adresse ist kein Feed. %@":"There's no feed at this address. %@",
"UwUNews startet nicht":"UwUNews can't start","Version":"Version","Vorheriger Artikel":"Previous article",
"Webseite oder Feed-Adresse":"Website or feed address","Webseite öffnen":"Open website",
"Welche Quellen und Sammlungen melden, stellst du an ihrer Glocke ein. iOS entscheidet, wie oft UwUNews im Hintergrund nachsieht – meist alle ein bis zwei Stunden, öfter, wenn du die App viel nutzt.":"Which sources and collections notify you is set with their bell. iOS decides how often UwUNews checks in the background – usually every hour or two, more often if you use the app a lot.",
"Wische einen Artikel nach links, um ihn für später zu merken.":"Swipe an article left to save it for later.",
"Wort oder Wortgruppe":"Word or phrase",
"Wörter passen auch als Anfang: „klima“ findet „Klimawandel“. Groß- und Kleinschreibung und Akzente zählen nicht.":"Words also match as a beginning: “climat” finds “climate”. Case and accents don't matter.",
"Zuletzt abgerufen":"Last fetched","Zum Aktualisieren nach unten ziehen.":"Pull down to refresh.",
"Zum Beispiel die Startseite einer Nachrichtenseite oder eines Blogs. UwUNews sucht den Feed selbst.":"For example the front page of a news site or a blog. UwUNews finds the feed itself.",
"Über":"About","Übersicht":"Overview","„%@“ löschen?":"Delete “%@”?","„%@“ entfernen?":"Remove “%@”?",
}
# key -> (de one, de other, en one, en other)
PLURAL = {
"%lld Quellen importiert":("%lld Quelle importiert","%lld Quellen importiert","Imported %lld source","Imported %lld sources"),
"%lld neue Artikel":("%lld neuer Artikel","%lld neue Artikel","%lld new article","%lld new articles"),
"%lld Artikel im Feed":("%lld Artikel im Feed","%lld Artikel im Feed","%lld article in the feed","%lld articles in the feed"),
"%lld Quellen":("%lld Quelle","%lld Quellen","%lld source","%lld sources"),
"%lld Filterwörter":("%lld Filterwort","%lld Filterwörter","%lld filter word","%lld filter words"),
}
INTS = {"count","notices.count","preview.itemCount","sourceIds.count","words","Int(count)","Int(preview.itemCount)"}
import re
def key(raw):
    raw = re.sub(r"\\\(Int\((.*?)\)\)", "%lld", raw)
    return re.sub(r'\\\((.*?)\)', lambda m: "%lld" if m.group(1) in INTS else "%@", raw)
def unit(v): return {"stringUnit":{"state":"translated","value":v}}
def plural(one, other): return {"variations":{"plural":{"one":unit(one),"other":unit(other)}}}
out = {}
missing = []
for raw in open(sys.argv[1]).read().splitlines():
    k = key(raw)
    if k in PLURAL:
        d1,d2,e1,e2 = PLURAL[k]
        out[k] = {"localizations":{"de":plural(d1,d2),"en":plural(e1,e2)}}
    elif k in EN:
        out[k] = {"localizations":{"en":unit(EN[k])}}
    else:
        missing.append(k)
out["·"] = {"shouldTranslate": False}
json.dump({"sourceLanguage":"de","strings":dict(sorted(out.items())),"version":"1.0"}, open(sys.argv[2],"w"), ensure_ascii=False, indent=2)
print("missing:", missing)
