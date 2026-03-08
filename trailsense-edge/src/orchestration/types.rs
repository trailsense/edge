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
    StartSniffing,
    StopSniffing,
    Connect,
    UploadData,
    SaveLocally,
    Sleep,
    Wake,
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub enum SystemEvents {
    InitComplete,
    Sniffer(SnifferEvents),
    Upload(UploadEvents),
    Data(DataEvents),
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
    UploadSuccessfull,
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
