from exchange.token_usage_collector import _TokenUsageCollector


def test_collect(usage_factory):
    usage_collector = _TokenUsageCollector()
    usage_collector.collect("model1", usage_factory(100, 1000, 1100))
    usage_collector.collect("model1", usage_factory(200, 2000, 2200))
    usage_collector.collect("model2", usage_factory(400, 4000, 4400))
    usage_collector.collect("model3", usage_factory(500, 5000, 5500))
    usage_collector.collect("model3", usage_factory(600, 6000, 6600))
    assert usage_collector.get_token_usage_group_by_model() == {
        "model1": usage_factory(300, 3000, 3300),
        "model2": usage_factory(400, 4000, 4400),
        "model3": usage_factory(1100, 11000, 12100),
    }


def test_collect_with_non_input_or_output_token(usage_factory):
    usage_collector = _TokenUsageCollector()
    usage_collector.collect("model1", usage_factory(100, None, None))
    usage_collector.collect("model1", usage_factory(None, 2000, None))
    assert usage_collector.get_token_usage_group_by_model() == {
        "model1": usage_factory(100, 2000, 0),
    }
