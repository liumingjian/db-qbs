import json
import os, sys
d = json.load(open(os.environ.get("OBS_JSON", "/tmp/v-obs.json")))
for k in sys.argv[1:]:
    print(f"===== {k} =====")
    print(json.dumps(d[k], ensure_ascii=False, indent=1))
