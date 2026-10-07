"""Text-only synchronization and persistent reference receiver."""
import json
import os
import threading
import uuid
from datetime import datetime, timezone
from pathlib import Path
from urllib.parse import urlsplit
from urllib.request import Request, build_opener, HTTPRedirectHandler
from dotenv import set_key
from . import database as db

_lock = threading.Lock()
ENV_PATH = Path(__file__).resolve().parent.parent / '.env'

def now():
    return datetime.now(timezone.utc).isoformat()

def utc(value, local=False):
    date = datetime.fromisoformat(value.replace('Z', '+00:00'))
    if date.tzinfo is None:
        date = date.astimezone() if local else date.replace(tzinfo=timezone.utc)
    return date.astimezone(timezone.utc).isoformat()

def initialize(conn):
    conn.executescript('''
    CREATE TABLE IF NOT EXISTS sync_config (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS sync_control (id INTEGER PRIMARY KEY, applying INTEGER NOT NULL);
    INSERT OR IGNORE INTO sync_control VALUES (1,0);
    CREATE TABLE IF NOT EXISTS sync_records (
      recording_id TEXT PRIMARY KEY, filename TEXT UNIQUE NOT NULL,
      updated_at TEXT NOT NULL, deleted_at TEXT, generation INTEGER NOT NULL DEFAULT 1,
      synced_generation INTEGER NOT NULL DEFAULT 0, revision INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS sync_conflicts (recording_id TEXT PRIMARY KEY, snapshot TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS sync_mock_records (recording_id TEXT PRIMARY KEY, snapshot TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS sync_mock_mutations (id TEXT PRIMARY KEY, acknowledgment TEXT NOT NULL);
    ''')
    for table in ('recording_metadata', 'transcriptions', 'recording_notes'):
        for action in ('INSERT', 'UPDATE', 'DELETE'):
            ref = 'old' if action == 'DELETE' else 'new'
            deletion = table == 'recording_metadata' and action == 'DELETE'
            conn.executescript(f'''
            CREATE TRIGGER IF NOT EXISTS sync_{table}_{action} AFTER {action} ON {table}
            WHEN {ref}.filename IS NOT NULL AND (SELECT applying FROM sync_control WHERE id=1)=0
            BEGIN
              INSERT INTO sync_records(recording_id,filename,updated_at,deleted_at)
              VALUES(lower(hex(randomblob(16))),{ref}.filename,strftime('%Y-%m-%dT%H:%M:%fZ','now'),
                {"strftime('%Y-%m-%dT%H:%M:%fZ','now')" if deletion else 'NULL'})
              ON CONFLICT(filename) DO UPDATE SET generation=generation+1,
                updated_at=excluded.updated_at, deleted_at={"excluded.deleted_at" if deletion or (table == 'recording_metadata' and action == 'INSERT') else 'sync_records.deleted_at'};
            END;
            ''')
    conn.execute('''INSERT OR IGNORE INTO sync_records(recording_id,filename,updated_at)
       SELECT lower(hex(randomblob(16))),filename,? FROM (
         SELECT filename FROM recording_metadata UNION SELECT filename FROM transcriptions
         UNION SELECT filename FROM recording_notes WHERE filename IS NOT NULL)''', (now(),))
    conn.execute("INSERT OR IGNORE INTO sync_config VALUES ('device_id',?)", (str(uuid.uuid4()),))
    conn.commit()

def config():
    return dict(db._get_conn().execute('SELECT key,value FROM sync_config'))

def put(key, value):
    db._get_conn().execute('INSERT OR REPLACE INTO sync_config VALUES (?,?)', (key, str(value)))

def settings():
    c = config()
    return {'enabled': c.get('enabled') == '1', 'endpoint': c.get('endpoint', ''),
            'mock': c.get('mock') == '1', 'api_key_configured': bool(os.getenv('GRABBY_SYNC_API_KEY')),
            'device_id': c['device_id'], 'last_success': c.get('last_success'),
            'error': c.get('error') or None, 'running': _lock.locked(),
            'conflicts': db._get_conn().execute('SELECT count(*) FROM sync_conflicts').fetchone()[0]}

def save_settings(enabled, endpoint, mock, api_key=None):
    endpoint = endpoint.strip()
    parsed = urlsplit(endpoint)
    if not mock and endpoint and (parsed.scheme != 'https' or not parsed.hostname or parsed.username or parsed.password or parsed.fragment or parsed.query):
        raise ValueError('Use an HTTPS endpoint without credentials, query or fragment.')
    key = api_key.strip() if api_key is not None else os.getenv('GRABBY_SYNC_API_KEY', '')
    if enabled and (not key or (not mock and not endpoint)):
        raise ValueError('An endpoint and API key are required to enable sync.')
    if '\r' in key or '\n' in key:
        raise ValueError('Invalid API key.')
    with _lock:
        if api_key is not None:
            set_key(str(ENV_PATH), 'GRABBY_SYNC_API_KEY', key)
            os.environ['GRABBY_SYNC_API_KEY'] = key
        c = config()
        with db._get_conn() as conn:
            if c.get('endpoint', '') != endpoint or (c.get('mock') == '1') != mock:
                conn.execute('UPDATE sync_records SET revision=0,synced_generation=0,generation=generation+1')
                conn.execute('DELETE FROM sync_conflicts')
                put('cursor', 0)
                put('last_success', '')
            put('enabled', int(enabled)); put('endpoint', endpoint); put('mock', int(mock))
            put('error', '')
    return settings()

def snapshot(row):
    filename = row['filename']
    metadata = db.get_metadata(filename)
    transcript = db.get_transcription(filename)
    notes = db._get_conn().execute('SELECT id,text FROM recording_notes WHERE filename=?', (filename,)).fetchone()
    return {'recording_id': row['recording_id'], 'filename': filename,
            'created_at': utc(metadata.get('created_at') or row['updated_at'], local=bool(metadata)),
            'updated_at': utc(row['updated_at']), 'deleted_at': row['deleted_at'],
            'base_revision': row['revision'], 'mutation_id': f"{row['recording_id']}:{row['generation']}",
            'title': metadata.get('title'),
            'transcription': {'text': transcript['text'], 'language': transcript['language'],
                              'created_at': utc(transcript['created_at'])} if transcript else None,
            'notes': dict(notes) if notes else None}

def validate_record(r, incoming=False):
    uuid.UUID(r['recording_id'])
    filename = r['filename']
    if not filename or Path(filename).name != filename or any(x in filename for x in '/\\:'):
        raise ValueError('Invalid filename')
    for key in ('created_at', 'updated_at'):
        utc(r[key])
    if r['deleted_at'] is not None:
        utc(r['deleted_at'])
    if r['title'] is not None and not isinstance(r['title'], str):
        raise ValueError('Invalid title')
    for key in ('transcription', 'notes'):
        if r[key] is not None and not isinstance(r[key]['text'], str):
            raise ValueError('Invalid text')
    if r['transcription'] is not None:
        utc(r['transcription']['created_at'])
    if r['notes'] is not None:
        uuid.UUID(r['notes']['id'])
    rev = r['revision'] if incoming else r['base_revision']
    if type(rev) is not int or rev < 0:
        raise ValueError('Invalid revision')
    if not incoming and (not isinstance(r['mutation_id'], str) or not r['mutation_id']):
        raise ValueError('Invalid mutation')

def receive(body):
    """Mock receiver transaction: account is the single configured local test account."""
    if body['schema_version'] != 1 or type(body['cursor']) is not int or body['cursor'] < 0:
        raise ValueError('Invalid sync request')
    uuid.UUID(body['device_id'])
    for r in body['records']:
        validate_record(r)
    conn = db._get_conn()
    acks, conflicts = [], []
    with conn:
        records = {r['recording_id']: json.loads(r['snapshot']) for r in conn.execute('SELECT * FROM sync_mock_records')}
        revision = max((r['revision'] for r in records.values()), default=0)
        if body['cursor'] > revision:
            raise ValueError('Cursor is ahead of this server')
        for r in body['records']:
            mutation = body['device_id'] + ':' + r['mutation_id']
            previous = conn.execute('SELECT acknowledgment FROM sync_mock_mutations WHERE id=?', (mutation,)).fetchone()
            if previous:
                acks.append(json.loads(previous[0])); continue
            current = records.get(r['recording_id'])
            if r['base_revision'] != (current['revision'] if current else 0):
                if current: conflicts.append(current)
                else: raise ValueError('Unknown recording with nonzero base revision')
                continue
            revision += 1
            saved = {k:r[k] for k in ('recording_id','filename','created_at','updated_at','deleted_at','title','transcription','notes')}
            saved['revision'] = revision
            records[r['recording_id']] = saved
            ack = {'recording_id': r['recording_id'], 'mutation_id': r['mutation_id'], 'revision': revision}
            conn.execute('INSERT OR REPLACE INTO sync_mock_records VALUES (?,?)', (r['recording_id'], json.dumps(saved)))
            conn.execute('INSERT INTO sync_mock_mutations VALUES (?,?)', (mutation, json.dumps(ack)))
            acks.append(ack)
        return {'schema_version': 1, 'cursor': revision, 'acknowledgments': acks,
                'changes': [r for r in records.values() if r['revision'] > body['cursor']], 'conflicts': conflicts}

class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None

def exchange(body, c):
    if c.get('mock') == '1':
        failure = c.get('mock_failure')
        if failure:
            with db._get_conn(): put('mock_failure', '')
            if failure == 'authentication':
                from urllib.error import HTTPError
                raise HTTPError('mock', 401, 'Invalid API key', {}, None)
            raise TimeoutError('Simulated mock failure')
        return receive(body)
    request = Request(c['endpoint'], data=json.dumps(body).encode(),
                      headers={'Content-Type': 'application/json', 'Authorization': 'Bearer ' + os.environ['GRABBY_SYNC_API_KEY']})
    with build_opener(NoRedirect()).open(request, timeout=20) as response:
        raw = response.read(10_000_001)
        if len(raw) > 10_000_000: raise ValueError('Sync response exceeds limit')
        return json.loads(raw)

def apply_remote(r):
    conn = db._get_conn()
    existing = conn.execute('SELECT * FROM sync_records WHERE recording_id=?', (r['recording_id'],)).fetchone()
    filename = existing['filename'] if existing else r['filename']
    collision = conn.execute('SELECT recording_id FROM sync_records WHERE filename=?', (filename,)).fetchone()
    from .recorder import RECORDINGS_DIR
    if not existing and (collision or (RECORDINGS_DIR / filename).exists()):
        path = Path(filename)
        filename = path.stem + '-' + r['recording_id'] + path.suffix
    conn.execute('UPDATE sync_control SET applying=1 WHERE id=1')
    try:
        conn.execute('DELETE FROM transcriptions WHERE filename=?', (filename,))
        conn.execute('DELETE FROM recording_notes WHERE filename=?', (filename,))
        conn.execute('DELETE FROM recording_metadata WHERE filename=?', (filename,))
        if not r['deleted_at']:
            conn.execute('INSERT INTO recording_metadata(filename,title,created_at) VALUES (?,?,?)', (filename,r['title'],r['created_at']))
            if r['transcription']:
                t = r['transcription']
                conn.execute('INSERT INTO transcriptions(filename,text,language,created_at) VALUES (?,?,?,?)', (filename,t['text'],t.get('language'),t['created_at']))
            if r['notes']:
                n = r['notes']
                # Notes IDs can collide across independently imported recordings; retain both texts.
                nid = n['id']
                if conn.execute('SELECT 1 FROM recording_notes WHERE id=?', (nid,)).fetchone(): nid = str(uuid.uuid4())
                conn.execute('INSERT INTO recording_notes VALUES (?,?,?)', (nid,filename,n['text']))
        conn.execute('''INSERT INTO sync_records VALUES (?,?,?,?,1,1,?)
          ON CONFLICT(recording_id) DO UPDATE SET updated_at=excluded.updated_at,
          deleted_at=excluded.deleted_at,revision=excluded.revision,synced_generation=generation''',
          (r['recording_id'],filename,r['updated_at'],r['deleted_at'],r['revision']))
    finally:
        conn.execute('UPDATE sync_control SET applying=0 WHERE id=1')

def run():
    if not _lock.acquire(blocking=False):
        raise ValueError('Sync is already running.')
    try:
        c = config()
        if c.get('enabled') != '1': raise ValueError('Enable sync in Settings first.')
        conn = db._get_conn()
        pending = [dict(r) for r in conn.execute('''SELECT * FROM sync_records WHERE generation>synced_generation
                    AND recording_id NOT IN (SELECT recording_id FROM sync_conflicts)''')]
        sent = [snapshot(r) for r in pending]
        body = {'schema_version':1,'device_id':c['device_id'],'cursor':int(c.get('cursor',0)), 'records':sent}
        response = exchange(body, c)
        if response['schema_version'] != 1 or type(response['cursor']) is not int or response['cursor'] < body['cursor']:
            raise ValueError('Invalid response cursor')
        for r in response['changes'] + response['conflicts']:
            validate_record(r, incoming=True)
            if r['revision'] <= 0 or r['revision'] > response['cursor']:
                raise ValueError('Invalid snapshot revision')
        mutations = {r['mutation_id']: r for r in sent}
        with conn:
            for ack in response['acknowledgments']:
                original = mutations.get(ack['mutation_id'])
                if not original or original['recording_id'] != ack['recording_id'] or type(ack['revision']) is not int or ack['revision'] <= original['base_revision'] or ack['revision'] > response['cursor']:
                    raise ValueError('Invalid acknowledgment')
                generation = next(r['generation'] for r in pending if r['recording_id']==ack['recording_id'])
                conn.execute('UPDATE sync_records SET synced_generation=?,revision=? WHERE recording_id=?', (generation,ack['revision'],ack['recording_id']))
            for r in response['conflicts'] + response['changes']:
                local = conn.execute('SELECT * FROM sync_records WHERE recording_id=?',(r['recording_id'],)).fetchone()
                if local and r['revision'] <= local['revision']: continue
                if local and local['generation'] > local['synced_generation']:
                    conn.execute('INSERT OR REPLACE INTO sync_conflicts VALUES (?,?)', (r['recording_id'],json.dumps(r)))
                else:
                    apply_remote(r)
            put('cursor',response['cursor']); put('last_success',now()); put('error','')
        return {'uploaded':len(response['acknowledgments']), 'downloaded':len(response['changes']), **settings(), 'running':False}
    except Exception as exc:
        # Never expose provider responses, URLs or credentials in logs/status.
        with db._get_conn(): put('error', 'Sync failed (' + type(exc).__name__ + '). Check connection and credentials.')
        raise
    finally:
        _lock.release()

def conflicts():
    result = []
    for row in db._get_conn().execute('SELECT * FROM sync_conflicts'):
        local = db._get_conn().execute('SELECT * FROM sync_records WHERE recording_id=?',(row['recording_id'],)).fetchone()
        result.append({'recording_id':row['recording_id'],'local':snapshot(local),'server':json.loads(row['snapshot'])})
    return result

def resolve(recording_id, choice):
    if choice not in ('local','server'): raise ValueError('Choose local or server.')
    with _lock, db._get_conn() as conn:
        row = conn.execute('SELECT snapshot FROM sync_conflicts WHERE recording_id=?',(recording_id,)).fetchone()
        if row is None: raise ValueError('Conflict not found.')
        remote = json.loads(row[0])
        if choice == 'server': apply_remote(remote)
        else:
            conn.execute('UPDATE sync_records SET revision=?,generation=generation+1,updated_at=? WHERE recording_id=?',(remote['revision'],now(),recording_id))
        conn.execute('DELETE FROM sync_conflicts WHERE recording_id=?',(recording_id,))
    return settings()


def mock_scenario(scenario):
    c = settings()
    if not c['mock'] or not c['enabled']:
        raise ValueError('Enable and save mock sync first.')
    if scenario in ('failure', 'authentication'):
        with _lock, db._get_conn(): put('mock_failure', scenario)
        return {'message': 'Next mock sync will fail once; retry to recover.'}
    if scenario not in ('remote_edit', 'conflict'):
        raise ValueError('Unknown mock scenario.')
    run()
    with _lock, db._get_conn() as conn:
        row = conn.execute("SELECT snapshot FROM sync_mock_records WHERE json_extract(snapshot,'$.deleted_at') IS NULL LIMIT 1").fetchone()
        if row is None: raise ValueError('Create a recording with text first.')
        remote = json.loads(row[0])
        remote['base_revision'] = remote.pop('revision')
        remote['mutation_id'] = str(uuid.uuid4())
        remote['updated_at'] = now()
        remote['title'] = (remote['title'] or Path(remote['filename']).stem) + ' (mock device edit)'
        receive({'schema_version':1,'device_id':str(uuid.uuid4()),'cursor':0,'records':[remote]})
        if scenario == 'conflict':
            local = conn.execute('SELECT filename FROM sync_records WHERE recording_id=?',(remote['recording_id'],)).fetchone()
            if local: db.set_title(local['filename'], 'Local conflicting title')
    return {'message':'Mock second device changed a recording. Sync now to receive it.'}
