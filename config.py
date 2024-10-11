import json

# Load configuration from file
with open('config.json') as f:
    config = json.load(f)

# Initialize health bar with config
health_bar = HealthBar(session_name="Goose Session", show_health_bar=config["show_health_bar"])
