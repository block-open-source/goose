# goose_session.py
from services.health_bar import HealthBar

def run_goose_session():
    health_bar = HealthBar(session_name="Goose Session", show_health_bar=True)
    health_bar.start_session()
    
    display_thread = threading.Thread(target=health_bar.display_health_bar)
    display_thread.start()

    # Simulate goose session logic
    for i in range(10):  # Example loop for token generation
        health_bar.add_tokens(i)
        health_bar.add_cost(i * 2)
        time.sleep(1)

    # Stop the session and health bar
    health_bar.stop_session()
    health_bar.stop_display()
    display_thread.join()
