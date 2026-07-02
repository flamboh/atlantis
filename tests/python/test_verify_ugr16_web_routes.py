import importlib.util
from pathlib import Path


def load_verifier():
    script_path = Path(__file__).resolve().parents[2] / 'scripts' / 'local' / 'verify_ugr16_web_routes.py'
    spec = importlib.util.spec_from_file_location('verify_ugr16_web_routes', script_path)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_dataset_summaries_accepts_apps_web_data_response() -> None:
    verifier = load_verifier()

    assert verifier.dataset_summaries({'data': [{'datasetId': 'ugr16'}], 'error': None}) == [
        {'datasetId': 'ugr16'}
    ]


def test_dataset_summaries_accepts_existing_shapes() -> None:
    verifier = load_verifier()

    assert verifier.dataset_summaries([{'datasetId': 'ugr16'}]) == [{'datasetId': 'ugr16'}]
    assert verifier.dataset_summaries({'datasets': [{'datasetId': 'ugr16'}]}) == [
        {'datasetId': 'ugr16'}
    ]
