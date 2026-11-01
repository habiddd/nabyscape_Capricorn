"""Builds train.jsonl, tools.json and exam.json for the Needle fine-tune. No network."""
import json, random, re, sys

random.seed(7)
ROUTER = sys.argv[1]
OUT = sys.argv[2]
src = open(ROUTER, encoding="utf-8").read()
tools = json.loads(re.search(r'pub const TOOLS: &str = r#"(.*?)"#;', src, re.S).group(1))
json.dump(tools, open(f"{OUT}/tools.json", "w"), indent=1)
plain = [{k: v for k, v in t.items() if k != "triggers"} for t in tools]

MEDIA = {
    "pause": ["pausa la musica", "pausa la cancion", "pon en pausa la musica", "pon pausa a la cancion", "pausar la musica",
              "deten la musica", "deten la cancion", "detener la musica", "pausa el video", "pausa el tema", "pausa",
              "pause the music", "pause the song", "pause the video", "pause", "pause this track", "pausa esto"],
    "play": ["reproduce la musica", "reproduce la cancion", "reanuda la musica", "reanuda la cancion", "reproducir la musica",
             "continua la musica", "sigue con la musica", "reproduce el video", "reanuda el video", "reproduce",
             "play the music", "play the song", "resume the music", "resume the song", "resume the video", "play", "resume"],
    "next": ["siguiente cancion", "la siguiente cancion", "pon la siguiente cancion", "pasa a la siguiente cancion", "salta esta cancion",
             "salta la cancion", "saltar esta cancion", "siguiente tema", "siguiente pista", "siguiente video", "siguiente",
             "next song", "next track", "skip this song", "skip the song", "skip this track", "next", "skip"],
    "previous": ["cancion anterior", "la cancion anterior", "pon la cancion anterior", "vuelve a la cancion anterior", "tema anterior",
                 "pista anterior", "video anterior", "anterior",
                 "previous song", "previous track", "the previous song", "go to the previous song", "previous", "previous video"],
}
PAGES = {
    "media": ["la musica", "musica", "the music", "media", "the media page"],
    "weather": ["el clima", "clima", "el tiempo", "the weather", "weather", "the weather page"],
    "calendar": ["el calendario", "calendario", "la agenda", "the calendar", "calendar", "my calendar"],
    "timer": ["el temporizador", "temporizador", "the timer", "the timer page"],
    "system": ["el sistema", "sistema", "the system page", "system"],
    "notes": ["las notas", "notas", "mis notas", "the notes", "notes", "my notes"],
}
OPEN = ["abre", "abrir", "muestra", "muestrame", "ensename", "open", "show", "show me"]
VOL = ["pon el volumen al {n}", "pon el volumen a {n}", "baja el volumen a {n}", "sube el volumen a {n}", "volumen al {n}", "volumen {n}",
       "deja el volumen en {n}", "ajusta el volumen a {n}", "set the volume to {n}", "volume {n}", "turn the volume to {n}", "change the volume to {n}"]
TIM = ["pon un temporizador de {n} minutos", "temporizador de {n} minutos", "inicia un temporizador de {n} minutos", "crea un temporizador de {n} minutos",
       "pon el temporizador en {n} minutos", "set a timer for {n} minutes", "start a timer for {n} minutes", "start a {n} minute timer", "timer for {n} minutes"]
WORDS = {1: "un", 2: "dos", 3: "tres", 4: "cuatro", 5: "cinco", 10: "diez", 15: "quince", 20: "veinte", 30: "treinta"}
MUTE = ["silencia", "silencia el sonido", "silencia el pc", "silencia la computadora", "silenciar el sonido", "mutea el sonido", "mutea",
        "quita el silencio", "activa el sonido", "mute", "mute the sound", "mute the computer", "unmute", "unmute the sound"]
NEG = """explicame que es la fotosintesis|cual es la capital de francia|escribeme un poema sobre el mar|como estas hoy|como te llamas|hola|gracias|muchas gracias
buenos dias|buenas noches|que hora es|que dia es hoy|cuentame un chiste|tell me a joke|hello|thank you|how are you|what is your name|what time is it
no quiero que pongas musica|no pauses la musica|no pongas el temporizador|no abras el clima|no cambies el volumen|no silencies el sonido|nunca pongas esa cancion
dont pause the music|do not open the calendar|never mute the sound|dont change the volume|no quiero musica ahora|no me pongas un temporizador
me gusta esta cancion|me encanta esta musica|odio esta cancion|esta cancion es muy buena|que buena musica|esa cancion me recuerda a mi madre|i love this song|this song is great
recomiendame una cancion|recomiendame musica para estudiar|que musica me recomiendas|cual es tu cancion favorita|quien canta esta cancion|de que trata esta cancion
what song is this|who sings this song|recommend me a song|recommend some music|que opinas de la musica de los ochenta|como se toca esta cancion en guitarra
cuanto dura un temporizador de cocina|para que sirve un temporizador|como funciona un temporizador|how long should i boil an egg|how does a timer work|cuanto tarda en hervir el agua
como esta el clima en madrid|va a llover manana|hace frio afuera|que tiempo hace en lima|what is the weather usually like in spain|is it going to rain tomorrow|me gusta el clima de aqui
por que sube el volumen de los anuncios|el volumen de esta pelicula es raro|como se mide el volumen de un cubo|what is the volume of a sphere|el volumen del libro dos es mejor
abre tu corazon y cuentame un secreto|muestrame como se hace una tortilla|ensename a programar|ensename ingles|show me how to cook rice|open your mind and tell me a story|abre la mente|muestrame un ejemplo de codigo
que tengo en mi calendario|cuando es mi proxima reunion|escribe una nota sobre gatos|que dicen mis notas|que es un sistema operativo|el sistema solar tiene ocho planetas|explain how the system works
la siguiente pregunta es dificil|cual es el siguiente paso|el capitulo anterior fue mejor|el ano anterior fue duro|what is the next step|the previous answer was wrong|next time tell me first
ayudame con mi tarea|traduce esto al ingles|resume este texto|escribe un correo a mi jefe|write an email to my boss|help me with my homework|translate this to spanish|summarize this article
cuanto es dos mas dos|what is two plus two|quien gano el mundial|who won the world cup|que puedes hacer|what can you do|cuentame una historia|tell me a story|estoy cansado|tengo hambre|i am tired
el silencio es importante|por que el silencio incomoda|que significa mute|the mute button is broken|mi amigo se quedo mudo""".replace("\n", "|").split("|")
EXTRA = ["", "", "", "por favor ", "oye ", "naby ", "hey naby ", "please "]
AFTER = ["", "", "", "", " por favor", " ya", " ahora", " please", " now"]


def line(query, answers, why):
    return {"query": query, "tools": plain, "answers": answers, "reasoning": why}


def vary(text):
    return (random.choice(EXTRA) + text + random.choice(AFTER)).strip()


rows = []
for action, phrases in MEDIA.items():
    for p in phrases:
        for _ in range(6):
            rows.append(line(vary(p), [{"name": "media_control", "arguments": {"action": action}}], f"'{p}' -> action {action}"))
for page, names in PAGES.items():
    for name in names:
        for verb in OPEN:
            q = f"{verb} {name}"
            rows.append(line(vary(q), [{"name": "open_page", "arguments": {"page": page}}], f"'{name}' -> page {page}"))
for t in VOL:
    for _ in range(14):
        n = random.choice(range(0, 101, 5)) if random.random() < 0.7 else random.randint(0, 100)
        rows.append(line(vary(t.format(n=n)), [{"name": "set_volume", "arguments": {"percent": n}}], f"'{n}' -> percent {n}"))
for t in TIM:
    for _ in range(14):
        n = random.randint(2, 90)
        rows.append(line(vary(t.format(n=n)), [{"name": "start_timer", "arguments": {"minutes": n}}], f"'{n} minutes' -> minutes {n}"))
    for n, w in WORDS.items():
        if n > 1 and "minutos" in t:
            rows.append(line(vary(t.format(n=w)), [{"name": "start_timer", "arguments": {"minutes": n}}], f"'{w}' -> minutes {n}"))
for p in MUTE:
    for _ in range(6):
        rows.append(line(vary(p), [{"name": "toggle_mute", "arguments": {}}], f"'{p}' -> mute"))
positives = len(rows)
# Refusals keep the tuned model from calling a tool on everything; Cactus warns about this.
for i in range(positives // 2):
    p = NEG[i % len(NEG)]
    rows.append(line(p if i < len(NEG) else vary(p), [], "No tool serves this request."))
random.shuffle(rows)
seen, unique = set(), []
for r in rows:
    if r["query"] not in seen:
        seen.add(r["query"])
        unique.append(r)
with open(f"{OUT}/train.jsonl", "w", encoding="utf-8") as f:
    for r in unique:
        f.write(json.dumps(r, ensure_ascii=False) + "\n")

# Never used in training: other wording, other numbers.
EXAM = [
    ("pon pausa a la musica", "media_control pause"), ("pausa esta cancion", "media_control pause"), ("detente musica", "media_control pause"), ("hold the music pause it", "media_control pause"),
    ("reanuda esta cancion", "media_control play"), ("reproduce esta musica", "media_control play"), ("play this song", "media_control play"), ("continua con la cancion", "media_control play"),
    ("pasa esta cancion", "media_control next"), ("siguiente musica", "media_control next"), ("salta este tema", "media_control next"), ("skip to the next song", "media_control next"),
    ("regresa a la cancion anterior", "media_control previous"), ("musica anterior", "media_control previous"), ("back to the previous track", "media_control previous"),
    ("volumen al 37", "set_volume 37"), ("ponme el volumen en 63", "set_volume 63"), ("sube volumen a 88", "set_volume 88"), ("baja volumen a 12", "set_volume 12"), ("put the volume at 45", "set_volume 45"),
    ("quiero un temporizador de 7 minutos", "start_timer 7"), ("temporizador 25 minutos", "start_timer 25"), ("ponme un temporizador de doce minutos", "start_timer 12"), ("make a timer for 45 minutes", "start_timer 45"),
    ("ve al clima", "open_page weather"), ("abreme el calendario", "open_page calendar"), ("muestra mis notas por favor", "open_page notes"), ("abre la pagina del sistema", "open_page system"), ("show the music page", "open_page media"), ("open my notes page", "open_page notes"),
    ("silencia el audio", "toggle_mute"), ("silencia los altavoces", "toggle_mute"), ("mutea la computadora", "toggle_mute"), ("mute the audio", "toggle_mute"),
    ("no quiero que pauses nada", None), ("no silencies el audio", None), ("no me abras las notas", None), ("do not skip this song", None),
    ("adoro esta musica", None), ("esta cancion me pone triste", None), ("this music is awful", None), ("sugiereme una cancion alegre", None), ("suggest a good song", None),
    ("quien compuso esta musica", None), ("cuando salio esta cancion", None), ("por que me gusta el silencio", None), ("que significa volumen", None), ("explica el volumen de un cilindro", None),
    ("como uso un temporizador de horno", None), ("lloverá el sabado", None), ("how hot is it in cairo", None), ("muestrame como cambiar una llanta", None), ("ensename frances", None),
    ("abre tu mente a nuevas ideas", None), ("el siguiente capitulo es largo", None), ("la version anterior era mejor", None), ("escribe un poema de amor", None), ("dame una receta de pan", None),
    ("cuantos planetas hay", None), ("como te sientes", None), ("buenas tardes", None), ("translate this sentence", None), ("write a function in python", None), ("estoy aburrido", None),
]
json.dump(EXAM, open(f"{OUT}/exam.json", "w", encoding="utf-8"), ensure_ascii=False, indent=0)
n = sum(1 for r in unique if not r["answers"])
print("train", len(unique), "refusals", n, "exam", len(EXAM), "overlap", [q for q, _ in EXAM if q in seen])
