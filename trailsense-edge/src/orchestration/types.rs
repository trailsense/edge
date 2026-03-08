#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CorrelationId(pub u32);

#[derive(PartialEq, Eq)]
pub enum SystemState {
    Idle,
    Sniffing,
    PreparingUpload,
    Connecting,
    Uploading,
    SavingData,
    Sleep,
}

#[derive(PartialEq, Eq)]
pub enum SystemCmd {
    StartSniffing { id: CorrelationId },
    StopSniffing { id: CorrelationId },
    Connect { id: CorrelationId },
    UploadData { id: CorrelationId },
    SaveLocally { id: CorrelationId },
    Sleep,
    Wake,
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum SystemEvents {
    Sniffer {
        id: CorrelationId,
        event: SnifferEvents,
    },
    Upload {
        id: CorrelationId,
        event: UploadEvents,
    },
    Data {
        id: CorrelationId,
        event: DataEvents,
    },
    InitComplete,
    Sleep(SleepEvents),
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum SnifferEvents {
    StartedSniffing,
    SniffingError,
    StoppedSniffing,
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum UploadEvents {
    NetworkConnected,
    NetworkError,
    UploadError,
    UploadSuccessful,
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum DataEvents {
    DataSaved,
    DataError,
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum SleepEvents {
    SleepFinished,
}
