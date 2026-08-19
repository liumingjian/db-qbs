import json
import os, sys
d = json.load(open(os.environ.get("OBS_JSON", "/tmp/x-obs.json")))
def dig(obj, path):
    for p in path.split("."):
        obj = obj[p]
    return obj
for k in sys.argv[1:]:
    print(f"===== {k} =====")
    print(json.dumps(dig(d, k), ensure_ascii=False, indent=1))
