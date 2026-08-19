import json
import os
d = json.load(open(os.environ.get("OBS_JSON", "/tmp/w-obs.json")))
for k in ("W1_W6_at_1440", "W2_at_1024"):
    v = d[k]
    print(k, {kk: v[kk] for kk in ("viewport","sections","section_boxes","reports_box",
                                   "columns","row_count","empty_suggestion_cells",
                                   "table_overflow_x","body_overflow_x","total_line")})
